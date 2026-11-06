//! The square: a unix socket where any other program reaches town, as a resident. The README's *at
//! the square*.
//!
//! - **from a shell**: `town '<first word>'`, saying what it is as its first word.
//! - **from another town**: the same, `town '{"name":"<domain>"}'`, by a name this town gives, as
//!   a join of that town's.

use std::io::{ErrorKind, Read, Write};
use std::os::unix::net::UnixStream as SyncStream;
use std::rc::Rc;
use std::sync::atomic::Ordering;

use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::UnixListener;

use crate::language::Language;
use crate::paths;
use crate::town::Town;
use crate::voice::{comes, say, Unsaid, Voice};
use crate::word::Word;

pub async fn run(square: UnixListener, town: Rc<Town>) {
    loop {
        match square.accept().await {
            Ok((stream, _)) => {
                let town = town.clone();
                tokio::task::spawn_local(async move {
                    at_square(stream, town.clone()).await;
                    town.went.notify_waiters();
                });
            }
            // one that gave up as it came is gone already: the next may come at once
            Err(e) if matches!(e.kind(), ErrorKind::ConnectionAborted | ErrorKind::Interrupted) => {
            }
            // none may come now, as when town has all the files it may open: until one goes
            Err(_) => town.went.notified().await,
        }
    }
}

/// Whoever comes says first what it is, a line of JSON, `{ kinds, own, speaks }`. Then it hears
/// its kinds, the present first, and each word it says goes in by the one door, never as town's
/// own. Any stream of bytes is a door: a socket, as the square's are, or one in memory; town asks
/// no more of it.
async fn at_square<S: AsyncRead + AsyncWrite + Unpin>(stream: S, town: Rc<Town>) {
    let (r, mut w) = tokio::io::split(stream);
    let who = town.who("square", true);
    // its error, as town makes any, told to whoever came, in the one language it surely hears
    let tell_it = async |w: &mut tokio::io::WriteHalf<S>, told: Word| {
        say(w, &told, None, &Language::Json, &town.languages, None).await.ok();
    };
    // one waiting for its language is told so here alone, at once, in JSON: it hears nothing else
    // until that language is spoken, and then hears from then
    let waits = async |message: &str| tell_it(&mut w, town.noted(&who, message.to_string())).await;
    let resident = match comes(&town, &who, Voice::new(r), waits).await {
        Ok(resident) => resident,
        // refused, it never comes: its error is said, and told it
        Err(Unsaid::Refused(e)) => {
            return tell_it(&mut w, town.failed(&who, None, e, None)).await;
        }
        Err(Unsaid::Mute | Unsaid::Gone) => return,
    };
    // heard and told at once: one that hangs up as it speaks, as `echo … | town '{}'` may,
    // cannot be told, but is heard
    resident.live(&town, &mut w, None::<std::future::Pending<()>>).await;
}

/// What a shell's one at the square waits on: the square's folder changed; bytes it read; what it
/// reads ended.
enum Comes {
    Changed,
    Read(Vec<u8>),
    Ended,
}

/// What it reads, each as it comes, then that it ended: read from the start, so one that waits for
/// the square still hears its input end.
fn reading(tx: std::sync::mpsc::Sender<Comes>) {
    std::thread::spawn(move || {
        let mut buf = vec![0; 1 << 16];
        loop {
            match std::io::stdin().read(&mut buf) {
                Ok(0) | Err(_) => return drop(tx.send(Comes::Ended)),
                Ok(n) => drop(tx.send(Comes::Read(buf[..n].to_vec()))),
            }
        }
    });
}

/// The square, from a shell, once a town is there: tried as its folder changes, as a town that
/// starts makes the square there. That it waits is said once. What it reads meanwhile is kept in
/// `read`. If what it reads ends having read nothing, it goes: 0; if it cannot wait, 1.
fn arrive(
    comes: &std::sync::mpsc::Receiver<Comes>,
    tx: std::sync::mpsc::Sender<Comes>,
    read: &mut Vec<u8>,
) -> Result<(SyncStream, bool), i32> {
    use notify::{RecursiveMode, Watcher};
    let path = paths::square();
    // watched before it is tried, so a town that comes between is not missed
    let mut watcher = notify::recommended_watcher(move |_: notify::Result<notify::Event>| {
        let _ = tx.send(Comes::Changed);
    })
    .map_err(|_| 1)?;
    let folder = path.parent().ok_or(1)?;
    watcher.watch(folder, RecursiveMode::NonRecursive).map_err(|_| 1)?;
    let (mut said, mut ended) = (false, false);
    loop {
        if let Ok(s) = SyncStream::connect(&path) {
            return Ok((s, ended));
        }
        if !std::mem::replace(&mut said, true) {
            eprintln!("town: waiting for a town at the square");
        }
        loop {
            match comes.recv().map_err(|_| 1)? {
                Comes::Changed => break,
                Comes::Read(bytes) => read.extend(bytes),
                // given nothing, it has nothing to say: done, 0; given words, it keeps them for
                // the town that comes
                Comes::Ended if read.is_empty() => return Err(0),
                Comes::Ended => ended = true,
            }
        }
    }
}

/// `town '<first word>'`: this program at the square, as its first word says it is, `{}` to talk,
/// `{"kinds":["/"]}` to hear, `{"name":"luke"}` to come by a name, as another town does. It waits
/// for a town to come there, as it does, then says its first word, and after it all it reads, and
/// writes all it is told, each as it comes. What it reads ending before a town comes is kept and
/// said when one comes. How it exits: 0 as what it reads ends; 1 as the square closes, or cannot
/// be waited for.
pub fn pipe(first: &str) -> i32 {
    let (tx, comes) = std::sync::mpsc::channel();
    reading(tx.clone());
    let mut read = Vec::new();
    let (mut to, ended) = match arrive(&comes, tx, &mut read) {
        Ok(arrived) => arrived,
        Err(code) => return code,
    };
    let Ok(from) = to.try_clone() else { return 1 };
    // all it read is said: the square that closes then closes as it should, not as it is lost
    let said = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let all_said = said.clone();
    if writeln!(to, "{first}").and_then(|_| to.write_all(&read)).is_err() {
        return 1;
    }
    // told, as it comes: the square closed, or lost, before all is said, it ends, 1; after, 0; one
    // that reads it gone, it ends quietly, 0
    std::thread::spawn(move || {
        let (mut from, mut buf) = (from, vec![0; 1 << 16]);
        let mut out = std::io::stdout();
        loop {
            let n = match from.read(&mut buf) {
                Ok(n @ 1..) => n,
                _ => std::process::exit(i32::from(!all_said.load(Ordering::SeqCst))),
            };
            if out.write_all(&buf[..n]).and_then(|()| out.flush()).is_err() {
                std::process::exit(0);
            }
        }
    });
    // said, as it comes, until what it reads ends: then what it said is heard, and it goes
    if !ended {
        for come in comes {
            match come {
                // the square closed: so its reader ends, 1, as it does
                Comes::Read(bytes) if to.write_all(&bytes).is_err() => return 1,
                Comes::Read(_) | Comes::Changed => {}
                Comes::Ended => break,
            }
        }
    }
    said.store(true, Ordering::SeqCst);
    let _ = to.shutdown(std::net::Shutdown::Write);
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::body::Body;
    use crate::residents::inside::speaking;
    use crate::testing::{guard, run, scratch};
    use crate::word::Tense;
    use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWriteExt};
    use tokio::net::UnixStream;

    /// the next line told, as a word
    async fn told(lines: &mut tokio::io::Lines<impl AsyncBufRead + Unpin>) -> Word {
        crate::voice::read(&guard(lines.next_line()).await.unwrap().unwrap()).unwrap()
    }

    /// one conversation at the square over `ours`, the other end `theirs` at its door: a peer that
    /// hears all and owns `mine` comes, is told the present and `now`, says a fact, wants, and says
    /// what it may not. What it is told, each word, whole: a test's town stamps by a count.
    async fn conversation<S>(
        mut ours: S,
        theirs: impl AsyncRead + AsyncWrite + Unpin + 'static,
    ) -> Vec<serde_json::Value>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        let dir = scratch();
        let town = Town::with_log(dir.log());
        town.talk(Word::new("theme", Tense::Present, Body::of("dark")));
        tokio::task::spawn_local(at_square(theirs, town.clone()));
        let (r, mut w) = tokio::io::split(&mut ours);
        let mut lines = tokio::io::BufReader::new(r).lines();
        let mut heard = Vec::new();
        let mut hear = async |n: usize| {
            for _ in 0..n {
                let line = guard(lines.next_line()).await.unwrap().expect("told");
                heard.push(serde_json::from_str::<serde_json::Value>(&line).unwrap());
            }
        };
        w.write_all(b"{\"kinds\":[\"/\",\"town/error\"],\"own\":[\"mine\"]}\n").await.unwrap();
        hear(3).await; // the present, its own fact among it, then `now`
        w.write_all(b"{\"kind\":\"mine\",\"tense\":\"present\",\"body\":1}\n").await.unwrap();
        hear(1).await;
        w.write_all(b"{\"kind\":\"theme\",\"tense\":\"future\",\"body\":\"light\"}\n")
            .await
            .unwrap();
        hear(1).await;
        w.write_all(b"{\"kind\":\"town/x\",\"tense\":\"present\",\"body\":1}\n").await.unwrap();
        hear(1).await; // refused: its error, as it has no other way to know
        heard
    }

    // the square asks nothing of its door but bytes: a conversation over a socket is the same, word
    // for word, as one over a stream in memory, so a transport may be swapped for another
    #[test]
    fn a_door_is_any_stream_of_bytes() {
        run(async {
            let (ours, theirs) = UnixStream::pair().unwrap();
            let socket = conversation(ours, theirs).await;
            let (ours, theirs) = tokio::io::duplex(64 * 1024);
            let memory = conversation(ours, theirs).await;
            let kinds: Vec<&str> = socket.iter().map(|w| w["kind"].as_str().unwrap()).collect();
            let me = "town/residents/square/1";
            assert_eq!(kinds, ["theme", me, "", "mine", "theme", "town/error"], "{socket:?}");
            assert_eq!(socket, memory, "the same, word for word");
        });
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

    // one that speaks and hangs up at once is heard, though it cannot be
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
            guard(there).await.unwrap();
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
    // them: so none takes another's refusal for its own
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

    // one at the square is told its own refusal: it has no other way
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
            // a's greeting, `by` it, then its refusal, `by` it, as one at the square hears its own
            let mut told_by = async || {
                let line = guard(lines.next_line()).await.unwrap().unwrap();
                crate::voice::said(&line).unwrap()
            };
            let (now, by) = told_by().await;
            assert!(now.is_now() && by.as_deref() == Some("a"), "its greeting");
            let (refused, by) = told_by().await;
            assert_eq!(refused.kind, "town/error", "told before what it said after");
            assert_eq!(
                (refused.body.value()["from"].as_str(), by.as_deref()),
                (Some("a"), Some("a"))
            );
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
            guard(there).await.unwrap();
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
            speaking(&town, &dir, "kv", r#"upto("\n")"#).await;
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
            // what it says is a kind alone, a fact of nothing: so a fact of each, for it to end
            for kind in ["first", "second"] {
                town.talk(Word::new(kind, Tense::Present, Body::of("was")));
            }
            let mut words = town.words.listen();
            speaking(&town, &dir, "x", r#"seq{ { kind = upto("\n") } }"#).await;
            let (mut ours, theirs) = UnixStream::pair().unwrap();
            tokio::task::spawn_local(at_square(theirs, town.clone()));
            ours.write_all(b"{\"speaks\":[\"x\",\"json\"]}\nfirst\n").await.unwrap();
            crate::testing::until(&mut words, |w| w.kind == "first").await;
            town.reopen();
            speaking(&town, &dir, "x", r#"seq{ { kind = upto(";") } }"#).await;
            ours.write_all(b"second;\n").await.unwrap();
            let said = crate::testing::until(&mut words, |w| w.kind.starts_with("second")).await;
            assert_eq!(said.kind, "second", "read in the language as it is now");
        });
    }
}
