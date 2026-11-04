//! The square: a unix socket where any other program reaches town, as a resident. The README's *at
//! the square*.
//!
//! - **from a shell**: `talk`, `ask` and `listen`, each saying what it is for it.

use std::io::Write;
use std::os::unix::net::UnixStream as SyncStream;
use std::rc::Rc;

use tokio::net::{UnixListener, UnixStream};

use crate::body::Body;
use crate::language::Language;
use crate::parts::Parts;
use crate::paths;
use crate::town::Town;
use crate::voice::{self, comes, say, Unsaid, Voice};
use crate::word::{Tense, Word};

pub async fn run(square: UnixListener, town: Rc<Town>) {
    loop {
        match square.accept().await {
            Ok((stream, _)) => _ = tokio::task::spawn_local(at_square(stream, town.clone())),
            // none may come now, as when town holds all the files it may: a moment, not a spin
            Err(_) => tokio::time::sleep(std::time::Duration::from_millis(100)).await,
        }
    }
}

// Whoever comes says first what it is, a line of JSON, `{ kinds, own, speaks }`. Then it hears its
// kinds, the present first, and each word it says goes in by the one door, never as town's own.
async fn at_square(stream: UnixStream, town: Rc<Town>) {
    let (r, mut w) = stream.into_split();
    let who = town.who("square", true);
    // told to whoever came, in the one language it surely hears
    let tell_it = async |w: &mut tokio::net::unix::OwnedWriteHalf, message: &str| {
        let told = Parts::map([("from", "square"), ("message", message)]);
        let told = Word::new("town/error", Tense::Past, Body::of(told));
        say(w, &told, &Language::Json, &town.languages).await.ok();
    };
    // one waiting for its language is told so here, at once, in JSON: it hears nothing else until
    // that language is spoken
    let waits = async |message: &str| tell_it(&mut w, message).await;
    let resident = match comes(&town, &who, Voice::new(r), waits).await {
        Ok(resident) => resident,
        Err(Unsaid::Refused(e)) => {
            tell_it(&mut w, &e).await;
            return town.error(&who, None, e);
        }
        Err(Unsaid::Mute | Unsaid::Gone) => return,
    };
    // heard and told at once: one that hangs up as it speaks, as `town talk` does, cannot be told,
    // but is heard
    resident.live(&town, &mut w, None::<std::future::Pending<()>>).await;
}

/// what one from a shell says it is: its kinds, and that it says JSON and hears `hears`
fn what_it_is(kinds: &[&str], hears: &str) -> String {
    serde_json::json!({ "kinds": kinds, "speaks": ["json", hears] }).to_string()
}

/// The square, from a shell, tried `tries` times a tenth of a second apart; if no one is there,
/// that is said.
fn reach(tries: u32) -> Option<SyncStream> {
    for i in 1..=tries {
        if let Ok(s) = SyncStream::connect(paths::square()) {
            return Some(s);
        }
        if i < tries {
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }
    eprintln!("town: no one at the square (is town up?)");
    None
}

/// `town talk`: say a word, and go, at once: one that waits for the square would pile up, hung.
pub fn talk(word: &Word) -> bool {
    let Some(mut s) = reach(1) else { return false };
    writeln!(s, "{}\n{}", what_it_is(&[], "json"), voice::line(word)).is_ok()
}

/// `town ask`: say a word, given an `id` if it has none, and print the body of its answer, the past
/// word of its kind and `id`. How it exits: 0 answered; 1 if it is refused, no town is at the
/// square, the square closes, or `wait` runs out; 2 if it cannot be asked (`asking`).
pub fn ask(word: Word, wait: std::time::Duration) -> i32 {
    use std::io::{BufRead, BufReader as SyncReader, ErrorKind};
    let now =
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    let fresh = format!("{}-{}", std::process::id(), now.as_millis());
    let (word, id) = match asking(word, fresh) {
        Ok(asked) => asked,
        Err(e) => {
            eprintln!("town: ask: {e}");
            return 2;
        }
    };
    let Some(mut s) = reach(1) else { return 1 };
    // it hears its kind before it asks, so the answer cannot come first; and, as any at the
    // square, its own errors, and no other's
    let (kind, sent) = (word.kind.as_str(), voice::line(&word));
    if writeln!(s, "{}\n{sent}", what_it_is(&[kind], "json")).is_err() {
        return 1;
    }
    let until = std::time::Instant::now() + wait;
    s.set_read_timeout(Some(std::time::Duration::from_millis(500))).ok();
    // a line is kept as it comes, however many reads it takes: a read that times out mid-line
    // loses nothing of it
    let (mut from, mut held) = (SyncReader::new(s), Vec::new());
    while std::time::Instant::now() < until {
        match from.read_until(b'\n', &mut held) {
            Ok(0) => return 1, // the square closed
            Ok(_) if held.ends_with(b"\n") => {
                let line = String::from_utf8_lossy(&held).into_owned();
                held.clear();
                let Ok(w) = voice::read(line.trim_end()) else { continue };
                let part = |name: &str| w.body.part(name).and_then(Parts::text).map(String::from);
                if w.kind == kind && w.tense == Tense::Past && part("id") == Some(id.clone()) {
                    println!("{}", String::from_utf8_lossy(&w.body.json()));
                    return 0;
                }
                // its own word refused, or not heard as a word: no answer is coming. What else it
                // is told of itself, falling behind, is no refusal
                let refused = w.body.part("heard").or(w.body.part("said")).is_some();
                if w.kind == crate::word::ERROR && refused {
                    eprintln!("town: ask: {}", part("message").unwrap_or("refused".into()));
                    return 1;
                }
            }
            Ok(_) => return 1, // the square closed mid-line
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => continue,
            Err(_) => return 1,
        }
    }
    1
}

/// The word given an `id`, `fresh` if it has none, and the id; or why it cannot be asked: it is
/// not future, so its answer, a past word, could be any other, its own among them; or its body
/// cannot carry an id.
fn asking(mut word: Word, fresh: String) -> Result<(Word, String), String> {
    if word.tense != Tense::Future {
        return Err(format!("an ask is future: `town ask future {}`", word.kind));
    }
    let mut body = match word.body.parts() {
        Parts::Map(_) => word.body.parts().clone(),
        Parts::None => Parts::map::<&str, Parts>([]),
        _ => return Err("an ask's body is parts by name, to carry its id".into()),
    };
    let id = body.get("id").and_then(Parts::text).map_or(fresh, String::from);
    body.set("id", id.as_str());
    word.body = Body::of(body);
    Ok((word, id))
}

/// `town listen [language] [for pattern…]`: hear the patterns after `for`, or every one, in the
/// language named, or JSON. It waits up to 3 seconds for the square to come up. How it exits: 0
/// once the square closes; 1 if no town is at the square; 2 for arguments it cannot read.
pub fn listen(args: &[String]) -> i32 {
    let (named, after) = args.split_at(args.iter().position(|n| n == "for").unwrap_or(args.len()));
    let kinds: Vec<&str> = match after.len() {
        // a person listening hears all of it, its errors too
        0 => vec!["/", "town/error"],
        1 => return no("`for` names the kinds it hears"),
        _ => after[1..].iter().map(String::as_str).collect(),
    };
    let language = match named {
        [] => "json",
        [one] => one.as_str(),
        _ => return no("one language, or none"),
    };
    let Some(mut s) = reach(30) else { return 1 };
    let _ = writeln!(s, "{}", what_it_is(&kinds, language));
    // the bytes as said, a binary language too; a reader gone, its pipe closed, ends it quietly
    let _ = std::io::copy(&mut s, &mut std::io::stdout());
    0
}

/// arguments `listen` cannot read: why, and 2
fn no(why: &str) -> i32 {
    eprintln!("town: listen: {why}");
    2
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::residents::inside::speaking;
    use crate::testing::{guard, run, scratch};
    use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWriteExt};

    /// the next line told, as a word
    async fn told(lines: &mut tokio::io::Lines<impl AsyncBufRead + Unpin>) -> Word {
        crate::voice::read(&lines.next_line().await.unwrap().unwrap()).unwrap()
    }

    // one listening that names its kinds hears those and what is under them, the present first, and
    // no other: `nvims/`, only what is under it
    #[test]
    fn one_listening_hears_the_kinds_it_names() {
        run(async {
            let dir = scratch();
            let town = Town::with_log(dir.log());
            let fact = |kind: &str| Word::new(kind, Tense::Present, Body::of(kind));
            for kind in ["nvims", "nvims/1", "theme"] {
                town.talk(fact(kind));
            }
            let (mut ours, theirs) = UnixStream::pair().unwrap();
            tokio::task::spawn_local(at_square(theirs, town.clone()));
            ours.write_all(b"{\"kinds\":[\"nvims/\",\"place\"]}\n").await.unwrap();
            let mut lines = tokio::io::BufReader::new(ours).lines();
            let kind = told(&mut lines).await.kind;
            assert_eq!(kind, "nvims/1", "the present of what it names, and no other");
            assert_eq!(told(&mut lines).await.kind, "", "then that what follows is now");
            for kind in ["theme", "nvims", "nvimsx", "place", "nvims/2/a"] {
                town.talk(Word::new(kind, Tense::Past, Body::none()));
            }
            assert_eq!(told(&mut lines).await.kind, "place");
            assert_eq!(told(&mut lines).await.kind, "nvims/2/a");
        });
    }

    // one that speaks and hangs up at once, as `town talk` does, is heard, though it cannot be
    // told: a word, and what is no word, as a `town/error` from the square
    #[test]
    fn one_that_speaks_and_goes_is_heard() {
        run(async {
            let dir = scratch();
            let town = Town::with_log(dir.log());
            let mut words = town.words.listen();
            let (mut ours, theirs) = UnixStream::pair().unwrap();
            let there = tokio::task::spawn_local(at_square(theirs, town.clone()));
            let said =
                b"{}\n{\"kind\":\"said\",\"tense\":\"past\"}\n{\"kind\":\"x\",\"label\":1}\n";
            ours.write_all(said).await.unwrap();
            drop(ours);
            there.await.unwrap();
            assert_eq!(words.try_recv().unwrap().kind, "said");
            let no = words.try_recv().unwrap();
            assert_eq!(
                (no.kind.as_str(), no.body.value()["from"].as_str()),
                ("town/error", Some("square"))
            );
            assert!(no.body.value()["message"].as_str().unwrap().contains("unknown field `label`"));
        });
    }

    // one at the square hears its own errors, whatever it names, and no other's but as it names
    // them: so `town ask` takes no other's refusal for its own
    #[test]
    fn one_at_the_square_hears_only_its_own_errors() {
        let dir = scratch();
        let town = Town::with_log(dir.log());
        let (asker, other) = (town.who("square", true), town.who("square", true));
        let asker = crate::town::Resident { kinds: vec!["q".into()], ..asker };
        let error = |by: u64| Word { by, ..Word::new("town/error", Tense::Past, Body::none()) };
        assert!(asker.hears(&town, &error(asker.id)), "its own");
        assert!(!asker.hears(&town, &error(other.id)), "not another's of the same name");
    }

    // one at the square is told its own refusal, as `town ask` waits to be: it has no other way
    #[test]
    fn one_at_the_square_hears_its_own_refusal() {
        run(async {
            let dir = scratch();
            let town = Town::with_log(dir.log());
            let (mut ours, theirs) = UnixStream::pair().unwrap();
            tokio::task::spawn_local(at_square(theirs, town.clone()));
            ours.write_all(b"{\"kinds\":[\"town/error\"]}\n").await.unwrap();
            let (r, mut w) = ours.into_split();
            let mut lines = tokio::io::BufReader::new(r).lines();
            assert_eq!(told(&mut lines).await.kind, "", "what follows is now");
            w.write_all(b"{\"kind\":\"town/work\",\"tense\":\"future\"}\n").await.unwrap();
            let refused = told(&mut lines).await.body.value();
            assert_eq!(
                (refused["from"].as_str(), refused["heard"].as_str()),
                (Some("square"), Some("town/work"))
            );
        });
    }

    // one at the square is told when a word it says by one it speaks for is refused: the error is
    // the speaker's, from the one spoken for
    #[test]
    fn one_at_the_square_hears_the_refusal_of_one_it_speaks_for() {
        run(async {
            let dir = scratch();
            let town = Town::with_log(dir.log());
            let (mut ours, theirs) = UnixStream::pair().unwrap();
            tokio::task::spawn_local(at_square(theirs, town.clone()));
            let a = b"{\"kind\":\"town/residents/a\",\"body\":{\"kinds\":[\"y\"]}}\n";
            ours.write_all(&[&b"{\"kinds\":[\"x\"]}\n"[..], a].concat()).await.unwrap();
            let (r, mut w) = ours.into_split();
            let mut lines = tokio::io::BufReader::new(r).lines();
            assert_eq!(told(&mut lines).await.kind, "", "what follows is now");
            let by = b"{\"kind\":\"town/work\",\"tense\":\"future\",\"by\":\"a\"}\n";
            w.write_all(&[&by[..], b"{\"kind\":\"x\",\"tense\":\"past\"}\n"].concat())
                .await
                .unwrap();
            let refused = guard(told(&mut lines)).await;
            assert_eq!(refused.kind, "town/error", "told before what it said after");
            assert_eq!(refused.body.value()["from"], "a");
        });
    }

    // what one at the square owns, only it says, while it is there, though the town opens again;
    // as it hangs up, it is let go
    #[test]
    fn one_at_the_square_owns_while_it_is_there() {
        run(async {
            let dir = scratch();
            let town = Town::with_log(dir.log());
            let (mut ours, theirs) = UnixStream::pair().unwrap();
            let there = tokio::task::spawn_local(at_square(theirs, town.clone()));
            ours.write_all(b"{\"own\":[\"focus\"]}\n").await.unwrap();
            let (r, mut w) = ours.into_split();
            let mut lines = tokio::io::BufReader::new(r).lines();
            assert_eq!(told(&mut lines).await.kind, "", "what follows is now");
            let other = town.who("other", false);
            let focus = Word::new("focus", Tense::Present, Body::of(1i64));
            assert!(town.may(&focus, &other).is_err(), "while it is there");
            town.reopen();
            assert!(town.may(&focus, &other).is_err(), "though the town opens again");
            w.write_all(b"{\"kind\":\"focus\",\"body\":1}\n").await.unwrap();
            drop((w, lines));
            there.await.unwrap();
            assert_eq!(town.present.borrow()["focus"], Body::of(1i64), "it said it");
            assert!(town.may(&focus, &other).is_ok(), "let go as it hangs up");
        });
    }

    // one at the square whose first line is not what it is, as a resident reads it, is told so, in
    // JSON; and one that names a language town does not speak is told, in JSON, that it waits
    #[test]
    fn one_at_the_square_is_told_in_json_what_holds_it_back() {
        run(async {
            let dir = scratch();
            let town = Town::with_log(dir.log());
            let asks = [
                ("{\"speaks\":\"cbro\"}\n", "no language is called cbro yet: it waits for one"),
                ("listen kv\n", "its first line must say what it is"),
                ("{\"hears\":[\"x\"]}\n", "unknown field `hears`"),
            ];
            for (asked, told_of) in asks {
                let (mut ours, theirs) = UnixStream::pair().unwrap();
                tokio::task::spawn_local(at_square(theirs, town.clone()));
                ours.write_all(asked.as_bytes()).await.unwrap();
                let told = told(&mut tokio::io::BufReader::new(ours).lines()).await;
                assert_eq!(told.kind, "town/error", "{asked}");
                assert!(told.body.value()["message"].as_str().unwrap().contains(told_of));
            }
        });
    }

    // one that says what is not JSON as it waits for its language, then hangs up, is gone, and
    // what it owned is let go
    #[test]
    fn one_that_hangs_up_as_it_waits_is_gone() {
        run(async {
            let dir = scratch();
            let town = Town::with_log(dir.log());
            let (mut ours, theirs) = UnixStream::pair().unwrap();
            let there = tokio::task::spawn_local(at_square(theirs, town.clone()));
            ours.write_all(b"{\"own\":[\"mine\"],\"speaks\":\"cbro\"}\n\xff\xfe\n").await.unwrap();
            drop(ours);
            guard(there).await.unwrap();
            let mine = Word::new("mine", Tense::Past, Body::none());
            assert!(town.may(&mine, &town.who("other", false)).is_ok(), "let go as it hangs up");
        });
    }

    // one that hears `town/error` is told once that its language is not said yet, as it waits, and
    // not again once it is told
    #[test]
    fn one_that_hears_errors_is_told_once_it_waits() {
        run(async {
            let dir = scratch();
            let town = Town::with_log(dir.log());
            let (mut ours, theirs) = UnixStream::pair().unwrap();
            tokio::task::spawn_local(at_square(theirs, town.clone()));
            let what = b"{\"kinds\":[\"/\",\"town/error\"],\"speaks\":[\"kv\",\"json\"]}\n";
            ours.write_all(what).await.unwrap();
            let mut lines = tokio::io::BufReader::new(ours).lines();
            let told_it = guard(told(&mut lines)).await.body.value();
            assert_eq!(told_it["message"], "no language is called kv yet: it waits for one");
            speaking(&town, &dir, "kv", r#"upto("\n")"#);
            town.talk(Word::new("ping", Tense::Past, Body::none()));
            loop {
                let w = guard(told(&mut lines)).await;
                assert_ne!(w.kind, "town/error", "told once: {:?}", w.body.value());
                if w.kind == "ping" {
                    break;
                }
            }
        });
    }

    // one at the square speaks its language as it is now: gone and said again, another grammar of
    // the name, what it says after is read in that one, not the one it came with
    #[test]
    fn one_at_the_square_speaks_its_language_as_it_is_now() {
        run(async {
            let dir = scratch();
            let town = Town::with_log(dir.log());
            let mut words = town.words.listen();
            speaking(&town, &dir, "x", r#"seq{ { kind = upto("\n") } }"#);
            let (mut ours, theirs) = UnixStream::pair().unwrap();
            tokio::task::spawn_local(at_square(theirs, town.clone()));
            ours.write_all(b"{\"speaks\":[\"x\",\"json\"]}\nfirst\n").await.unwrap();
            crate::testing::until(&mut words, |w| w.kind == "first").await;
            town.reopen();
            speaking(&town, &dir, "x", r#"seq{ { kind = upto(";") } }"#);
            ours.write_all(b"second;\n").await.unwrap();
            let said = crate::testing::until(&mut words, |w| w.kind.starts_with("second")).await;
            assert_eq!(said.kind, "second", "read in the language as it is now");
        });
    }

    // an ask's body is parts by name, to carry its id: one that is not is refused
    #[test]
    fn an_ask_carries_its_id_in_parts() {
        let asked = |line: &str| asking(voice::read(line).unwrap(), "fresh".into());
        let (said, id) = asked(r#"{"kind":"q","tense":"future","body":{"a":1}}"#).unwrap();
        assert!(voice::line(&said).contains(r#""id":"fresh""#));
        assert_eq!((said.kind.as_str(), id.as_str()), ("q", "fresh"));
        let mine = r#"{"kind":"q","tense":"future","body":{"id":"mine"}}"#;
        assert_eq!(asked(mine).unwrap().1, "mine");
        let nothing = r#"{"kind":"q","tense":"future"}"#;
        assert!(asked(nothing).is_ok(), "nothing, to carry it in its place");
        let refused = asked(r#"{"kind":"q","tense":"future","body":"x"}"#).err();
        assert_eq!(refused.as_deref(), Some("an ask's body is parts by name, to carry its id"));
    }

    // an ask is future: one not, present by default, or past, which its own word would answer, is
    // refused, and why
    #[test]
    fn an_ask_is_future() {
        for line in [r#"{"kind":"q"}"#, r#"{"kind":"q","tense":"past"}"#] {
            let refused = asking(voice::read(line).unwrap(), "fresh".into()).err();
            assert_eq!(refused.as_deref(), Some("an ask is future: `town ask future q`"), "{line}");
        }
    }

    // listen exits 2 for arguments it cannot read, before it looks for a town
    #[test]
    fn listen_refuses_what_it_cannot_read() {
        let args = |a: &[&str]| a.iter().map(|a| a.to_string()).collect::<Vec<_>>();
        assert_eq!(listen(&args(&["for"])), 2, "`for` with nothing");
        assert_eq!(listen(&args(&["json", "kv"])), 2, "two languages");
    }
}
