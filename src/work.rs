//! What town is asked to do: its own futures, each answered in the past with how it went and what
//! was sent with it. The README's *town*.
//!
//! - **`town/work`**: a command, run without holding up town.
//! - **`town/after`**: a wait of `ms`.
//! - **`town/join`**, **`town/connect`**: talk with a program, or a socket, at a kind, until it
//!   ends.

use std::rc::Rc;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncWrite};

use crate::body::Body;
use crate::parts::Parts;
use crate::program::{Ends, Group};

use crate::town::{Joined, Listening, Resident, Town};
use crate::voice::{Life, Speaks, Voice};
use crate::word::{is_kind, Tense, Word, KIND};

/// A future `town/<verb>`, begun as `Town::talk` says it, so each is answered in the order asked,
/// and a join hears what is wanted of it after. A `town/open` goes to the town's `Opening`, if it
/// is kept. A `town/work` and a `town/after` are town's own, not a resident's: they outlive a
/// reopen. A join or a connect is spoken for by `by`, who asked for it. Any other verb, a typo or
/// what town says, is a `town/error`.
pub(crate) fn begin(town: &Rc<Town>, verb: &str, body: Body, by: u64) {
    let kind = format!("town/{verb}");
    let fail = |why: String| town.error(&Resident::town(), Some(&kind), why);
    match verb {
        "work" => match body.part("cmd").and_then(Parts::text).map(String::from) {
            Some(cmd) => _ = tokio::task::spawn_local(done(town.clone(), cmd, body)),
            None => fail("a work needs `cmd`, the command to run".into()),
        },
        // ms as any number: Lua's `/` makes a float of every division. A clock takes none below 0,
        // and none too big to count
        "after" => {
            let ms = body.part("ms").and_then(Parts::number);
            match ms.and_then(|ms| Duration::try_from_secs_f64(ms / 1000.0).ok()) {
                Some(wait) => _ = tokio::task::spawn_local(waited(town.clone(), wait, body)),
                None => fail("a wait needs ms: 0 or more".into()),
            }
        }
        "join" | "connect" => {
            if let Err(why) = join(town, verb, body, by) {
                fail(why);
            }
        }
        "open" => {
            let opening = town.opening.borrow().clone();
            if let Some(opening) = opening {
                opening.ask(town, &body);
            }
        }
        _ => fail(format!("town has no `{verb}`")),
    }
}

/// What a kind is joined to: a program it runs, or a socket it reaches, a path or a host:port.
enum Reach {
    Run(Vec<String>),
    At(String),
}

/// Join a program, or connect a socket, as a resident at the kind `as`, owning it and what is under
/// it while it runs. It speaks `speaks`, or JSON; its kinds are paths from `as`: `a/b` is
/// `as/a/b`, and none is `as`. One a resident asks for, numbered `by`, is under its place, spoken
/// for by it, so it goes as that one goes. Joining a kind again ends the one before, as a reopen
/// does. Why it cannot begin, if it cannot.
fn join(town: &Rc<Town>, verb: &str, body: Body, by: u64) -> Result<(), String> {
    let kind = body.part("as").and_then(Parts::text).map(String::from);
    if let Some(kind) = kind.as_deref().filter(|k| !is_kind(k)) {
        return Err(format!("`{kind}` is no kind: {KIND}"));
    }
    let reach = match verb {
        // a number in it is the text it is written as: `sleep 5`
        "join" => match body.part("argv").and_then(Parts::list) {
            Some(l) => {
                let text = |p: &Parts| match p {
                    Parts::Text(t) | Parts::Number(t) => Some(t.clone()),
                    _ => None,
                };
                let argv = l.iter().map(text).collect::<Option<Vec<_>>>();
                let argv = argv.ok_or("an `argv` is text and numbers")?;
                Some(argv).filter(|argv| !argv.is_empty()).map(Reach::Run)
            }
            None => None,
        },
        _ => body.part("to").and_then(Parts::text).map(|to| Reach::At(to.into())),
    };
    let (Some(kind), Some(reach)) = (kind, reach) else {
        let needs = if verb == "join" { "`argv` to run" } else { "`to`, a path or a host:port" };
        return Err(format!("a {verb} needs `as`, and {needs}"));
    };
    // town's own words mean what town makes them mean: a program cannot speak as one
    if crate::town::towns(&kind) {
        return Err(format!("`{kind}` is town's own"));
    }
    let speaks = body.part("speaks").map(Speaks::of).transpose()?;
    // its kind, named alone and as a tree, so none owns it nearer than it does
    let own = vec![kind.clone(), format!("/{kind}")];
    let who = Resident { at: Some(kind.clone()), own, speaks, ..town.who(&kind, false) };
    // asked by a resident here, it is under that one's place, and goes as it goes
    let asker = town.place_of(by).map(|place| (format!("{place}/{kind}"), by));
    let who = match asker {
        Some((placed, by)) => Resident { placed: Some(placed), by: Some(by), ..who },
        None => who,
    };
    // one that cannot come to its kind, or own it, cannot say what is there: it ends as it begins,
    // and the one before stays; it is answered, as any join is as it ends
    let before = town.joins.borrow().get(&kind).map(|j| j.who.id);
    if let Err(e) = town.may_come(&who, before) {
        town.error(&who, Some(&kind), e.clone());
        let body = answered(body, |m| {
            m.set("code", -1i64);
            m.set("out", e);
        });
        town.talk(Word::new(format!("town/{verb}"), Tense::Past, body));
        return Ok(());
    }
    // the one before ends, and lets go, so this one owns its kind in its place
    // taken out, then let go, so what its going says finds `joins` free to look at
    let before = town.joins.borrow_mut().remove(&kind);
    drop(before);
    let here = match town.come(&who) {
        Ok(here) => here,
        Err(e) => {
            town.error(&who, Some(&kind), e);
            return Ok(());
        }
    };
    // it hears from now what is wanted there, though it runs, and says its language, later
    let words = who.listening(town);
    let it = Join { town: town.clone(), resident: who.clone() };
    // among what runs for it, so it ends as it goes, and no longer holds the town
    let run = tokio::task::spawn_local(it.run(reach, body, words));
    let ends = Ends(run.abort_handle());
    town.runs(who.id, run);
    town.joins.borrow_mut().insert(kind, Joined { who, _here: here, _run: ends });
    Ok(())
}

/// A kind joined to what it talks with.
struct Join {
    town: Rc<Town>,
    /// it, standing at its kind, in the language it speaks: waited for, if not said yet
    resident: Resident,
}

impl Join {
    /// From its start to its end. As it ends, its kind is let go, and its past `town/join` or
    /// `town/connect` says how. One ended by a join again or a reopen says nothing: another stands
    /// in its place, and one that joined again as a join ends would join without end.
    async fn run(self, reach: Reach, body: Body, words: Listening) {
        let (verb, (code, out)) = match reach {
            Reach::Run(argv) => ("join", self.ran(&argv, words).await),
            Reach::At(to) => match self.reached(&to, words).await {
                Ok(()) => ("connect", (0, None)),
                Err(e) => ("connect", (-1, Some(format!("cannot connect to {to}: {e}")))),
            },
        };
        let (town, who) = (&self.town, &self.resident);
        let at = who.at.as_deref().unwrap_or_default();
        // its own, taken out, so its kind is let go: the run that ends is this one, which waits
        // for nothing more
        let mine = town.joins.borrow().get(at).is_some_and(|j| j.who.id == who.id);
        let joined = mine.then(|| town.joins.borrow_mut().remove(at));
        drop(joined);
        // one that cannot start says why, as any resident that fails does
        if let Some(why) = &out {
            town.error(who, Some(&format!("town/{verb}")), why.clone());
        }
        let body = answered(body, |m| {
            m.set("code", i64::from(code));
            if let Some(out) = out {
                m.set("out", out);
            }
        });
        town.talk(Word::new(format!("town/{verb}"), Tense::Past, body));
    }

    /// a program run, and talked with: how it ended
    async fn ran(&self, argv: &[String], words: Listening) -> (i32, Option<String>) {
        // a reopen, or a join again, ends the run, and the program with all it started; it lives
        // until it ends: one that stops saying is still told
        let who = &self.resident;
        let ran = Group::run(&self.town, who, argv, None, async |child, out, inp| {
            self.converse(out, inp, words, Some(child.wait())).await;
            child.wait().await
        });
        match ran.await {
            Ok(status) => (status.ok().and_then(|s| s.code()).unwrap_or(-1), None),
            Err(e) => (-1, Some(format!("cannot join {}: {e}", argv[0]))),
        }
    }

    /// a socket reached, and talked with, until it closes: a path, or a host:port
    async fn reached(&self, to: &str, words: Listening) -> std::io::Result<()> {
        if to.contains('/') {
            let (r, w) = tokio::net::UnixStream::connect(to).await?.into_split();
            self.converse(r, w, words, None::<std::future::Pending<()>>).await;
        } else {
            let (r, w) = tokio::net::TcpStream::connect(to).await?.into_split();
            self.converse(r, w, words, None::<std::future::Pending<()>>).await;
        }
        Ok(())
    }

    /// Both ways at once, as any resident lives (`live`), in its language once it is said, until a
    /// program ends or a socket hangs up.
    async fn converse(
        &self,
        out: impl AsyncRead + Unpin,
        mut inp: impl AsyncWrite + Unpin,
        words: Listening,
        end: Option<impl std::future::Future>,
    ) {
        let town = &self.town;
        let waits = async |m: &str| town.error(&self.resident, None, m.into());
        let (resident, voice) = (self.resident.clone(), Voice::new(out));
        let languages = self.resident.languages();
        let speaking = Life::speaking(town, resident, words, voice, languages, waits);
        if let Some(resident) = speaking.await {
            resident.live(town, &mut inp, end).await;
        }
    }
}

/// The caller's body with what town has to say of it, if it has parts to say it among: the
/// caller's own fields ride back with the answer, so it can match on them (`on`'s `by`).
fn answered(body: Body, say: impl FnOnce(&mut Parts)) -> Body {
    match body.parts() {
        Parts::Map(_) => {
            let mut parts = body.parts().clone();
            say(&mut parts);
            Body::of(parts)
        }
        _ => body,
    }
}

/// The time asked for, gone: said back as a past `town/after`, with everything the caller sent.
async fn waited(town: Rc<Town>, wait: Duration, body: Body) {
    tokio::time::sleep(wait).await;
    town.talk(Word::new("town/after", Tense::Past, body));
}

/// A command run by the world's `sh`: the past `town/work` carries all it was asked with, `cmd`
/// too, and what it said, how it ended, and its ms. One that cannot start says why, as a
/// `town/error` and in `out`, as a join does.
async fn done(town: Rc<Town>, cmd: String, body: Body) {
    let began = std::time::Instant::now();
    let out = match crate::program::command(&town, &["sh", "-c", &cmd]) {
        Ok(mut sh) => sh.output().await,
        Err(e) => Err(e),
    };
    let ms = began.elapsed().as_millis() as u64;
    let (code, stdout) = match out {
        Ok(o) => (
            o.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&o.stdout).trim_end().to_string(),
        ),
        Err(e) => {
            let why = format!("cannot run sh: {e}");
            town.error(&Resident::town(), Some("town/work"), why.clone());
            (-1, why)
        }
    };
    let body = answered(body, |m| {
        m.set("code", i64::from(code));
        m.set("out", stdout);
        m.set("ms", ms);
    });
    town.talk(Word::new("town/work", Tense::Past, body));
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    use crate::residents::inside::{speaking, wired};
    use crate::testing::{an_error, run, scratch, until, upto, Scratch};

    /// a town of a test's own, and all it says from now
    fn joining() -> (Scratch, Rc<Town>, Listening) {
        let dir = scratch();
        let town = Town::with_log(dir.log());
        let words = town.words.listen();
        (dir, town, words)
    }

    /// what is wanted of a kind, as a resident asks it
    fn want(town: &Town, kind: &str, body: Value) {
        town.talk(Word::new(kind, Tense::Future, Body::of(body)));
    }

    /// `line`, town's own language for a join that says lines, here as a town opens it
    fn line(town: &Rc<Town>) {
        wired(town, &crate::residents::own().join("languages/line.lua"));
    }

    /// `said`, a language as `line` is, but that a line read is the body `{ said }`
    fn said(town: &Rc<Town>, dir: &std::path::Path) {
        let tense = r#"{ tense = choice{ be("", "past"), be("", "future") } }"#;
        let body = r#"{ body = seq{ { said = upto("\n") } } }"#;
        speaking(town, dir, "said", &format!("seq{{ {tense}, {body} }}"));
    }

    // a joined program owns its kind while it runs and talks both ways; as it ends, the past
    // `town/join` says how, with what was sent. Joined again, the one before ends and says nothing.
    // One that cannot start says so as it ends; a number in argv is its text; and a join with
    // nothing to run, an argv of anything else, or as one of town's own words, is an error
    #[test]
    fn a_join_talks_until_it_ends() {
        run(async {
            let (_dir, town, mut words) = joining();
            line(&town);
            let join = |argv: Value, about: &str| {
                let join = json!({ "as": "cat", "argv": argv, "about": about, "speaks": "line" });
                want(&town, "town/join", join)
            };
            join(json!(["cat"]), "first");
            assert!(
                town.may(&Word::new("cat", Tense::Past, Body::none()), &town.who("square", true))
                    .is_err(),
                "the join's, while it runs"
            );
            let wanted = Word::new("cat/x", Tense::Future, Body::none());
            assert!(town.may(&wanted, &town.who("square", true)).is_err(), "to want it is town's");
            assert!(town.may(&wanted, &town.who("resident", false)).is_ok());
            want(&town, "cat", json!("hi"));
            until(&mut words, |w| w.kind == "cat" && w.body == "hi").await;
            // joined again: the first ends, unheard, and only the second is told
            join(json!(["sh", "-c", "read -r l; echo got $l; exit 3"]), "second");
            want(&town, "cat", json!("x"));
            let said = upto(&mut words, |w| w.kind == "town/join" && w.tense == Tense::Past).await;
            let back: Vec<&Body> = said
                .iter()
                .filter(|w| w.kind == "cat" && w.tense == Tense::Past)
                .map(|w| &w.body)
                .collect();
            assert_eq!(back, [&Body::of("got x")], "the first, told nothing more");
            let ended = said.last().unwrap();
            assert_eq!(
                (ended.body.value()["about"].as_str(), ended.body.value()["code"].as_i64()),
                (Some("second"), Some(3))
            );
            assert!(
                town.may(&Word::new("cat", Tense::Past, Body::none()), &town.who("square", true))
                    .is_ok(),
                "let go as it ends"
            );
            join(json!(["/no/such/program"]), "none");
            let said = upto(&mut words, |w| w.kind == "town/join" && w.tense == Tense::Past).await;
            let why = said.iter().find(|w| w.kind == "town/error").expect("it says why");
            assert!(why.body.value()["message"].as_str().unwrap().contains("/no/such/program"));
            let none = said.last().unwrap();
            assert_eq!(none.body.value()["code"], -1);
            assert!(none.body.value()["out"].as_str().unwrap().contains("/no/such/program"));
            join(json!([]), "empty");
            let bad = until(&mut words, |w| w.kind == "town/error").await;
            assert_eq!(bad.body.value()["heard"], "town/join");
            join(json!(["sh", "-c", "exit $0", 4]), "numbered");
            let ended =
                until(&mut words, |w| w.kind == "town/join" && w.tense == Tense::Past).await;
            assert_eq!(ended.body.value()["code"], 4);
            join(json!(["sh", {}]), "odd");
            let bad = until(&mut words, |w| w.kind == "town/error").await;
            assert_eq!(bad.body.value()["message"], "an `argv` is text and numbers");
            want(&town, "town/join", json!({ "as": "town/work", "argv": ["cat"] }));
            let bad = until(&mut words, |w| w.kind == "town/error").await;
            assert!(bad.body.value()["message"].as_str().unwrap().contains("town's own"));
            town.reopen();
        });
    }

    // a join is told the facts at its kind, by their paths from it, and none beside it, then that
    // what follows is now; what it says is said at its kind, and what names no kind is refused; it
    // is told the future words there, and its stderr is heard, in its own time
    #[test]
    fn a_join_speaks_at_its_kind() {
        run(async {
            let (_dir, town, _) = joining();
            town.talk(Word::new("box", Tense::Present, Body::of(0i64)));
            town.talk(Word::new("box/x", Tense::Present, Body::of(1i64)));
            town.talk(Word::new("boxes", Tense::Present, Body::of(2i64)));
            // what it says, and, heard apart, since a pipe beside it keeps no order with it, what
            // it says aside
            let (mut words, mut aside) = (town.words.listen(), town.words.listen());
            // what it is told, said back as it was told it: one told `box` would say `box/box`
            let program = r#"read -r root; read -r x; read -r joined
                printf '%s\n%s\n' "$root" "$x"
                printf '{"kind":"../out","tense":"past","body":null}\n'
                [ "$joined" = '{"kind":"","tense":"past","body":null}' ] &&
                  printf '{"kind":"joined","tense":"past","body":"yes"}\n'
                printf '{"kind":"again","tense":"future","body":null}\n'
                echo "said aside" >&2
                while read -r told; do printf '%s\n' "$told" | sed 's/"future"/"past"/'; done"#;
            want(&town, "town/join", json!({ "as": "box", "argv": ["sh", "-c", program] }));
            let said = |kind: &'static str| move |w: &Word| w.kind == kind;
            assert_eq!(until(&mut words, said("box")).await.body, Body::of(0i64));
            assert_eq!(until(&mut words, said("box/x")).await.body, Body::of(1i64));
            let refused = until(&mut words, an_error("is no kind")).await;
            assert!(refused.body.value()["message"].as_str().unwrap().contains("`box/../out`"));
            assert_eq!(until(&mut words, said("box/joined")).await.body, Body::of("yes"));
            // a future word under it is its own to be told, not to say
            let again = until(&mut words, an_error("is told to it")).await;
            assert_eq!(again.body.value()["heard"], "box/again");
            let aside = until(&mut aside, an_error("said aside")).await;
            assert_eq!(aside.body.value()["from"], "box");
            assert!(
                town.may(
                    &Word::new("box/x", Tense::Present, Body::none()),
                    &town.who("square", true)
                )
                .is_err(),
                "its own"
            );
            want(&town, "box/q", json!("w"));
            let back = until(&mut words, |w| w.kind == "box/q" && w.tense == Tense::Past).await;
            assert_eq!(back.body, Body::of("w"));
            town.reopen();
        });
    }

    // a join is told the present of its kind as it speaks, not before: one that says much before
    // it reads, its greeting more than a pipe holds, is heard and told both
    #[test]
    fn a_long_greeting_does_not_stop_a_join() {
        run(async {
            let (_dir, town, _) = joining();
            let long = "x".repeat(200);
            for n in 0..2000 {
                town.talk(Word::new(format!("big/{n}"), Tense::Present, Body::of(long.as_str())));
            }
            let mut words = town.words.listen();
            // 2000 lines said before one is read; then its greeting read, 2000 facts and `now`
            let program = r#"i=0; while [ $i -lt 2000 ]; do
                  echo '{"kind":"said","tense":"past","body":null}'; i=$((i+1)); done
                echo '{"kind":"done","tense":"past","body":null}'
                head -n 2001 > /dev/null; echo '{"kind":"told","tense":"past","body":null}'"#;
            want(&town, "town/join", json!({ "as": "big", "argv": ["sh", "-c", program] }));
            until(&mut words, |w| w.kind == "big/done").await;
            until(&mut words, |w| w.kind == "big/told").await;
            town.reopen();
        });
    }

    // a join hears from the moment it is asked for: what is wanted of it before it runs, or while
    // it waits for its language, is told it once it does
    #[test]
    fn a_join_is_told_what_is_wanted_as_it_starts() {
        run(async {
            let (_dir, town, mut words) = joining();
            want(&town, "town/join", json!({ "as": "early", "argv": ["cat"], "speaks": "line" }));
            want(&town, "early", json!("hi"));
            line(&town); // said after: it waited for it
            let said = |w: &Word| w.kind == "early" && w.tense == Tense::Past;
            assert_eq!(until(&mut words, said).await.body, Body::of("hi"));
            town.reopen();
        });
    }

    // joins within joins: each says what is at its kind, the nearest owning it, and the one
    // around it may not say it in its place, though it may want it
    #[test]
    fn a_join_within_a_join_says_its_own() {
        run(async {
            let (_dir, town, mut words) = joining();
            let inner = r#"echo '{"tense":"past","body":"mine"}'
                while read -r w; do printf '%s\n' "$w" | sed 's/"future"/"past"/'; done"#;
            want(&town, "town/join", json!({ "as": "box/cat", "argv": ["sh", "-c", inner] }));
            let mine = until(&mut words, |w| w.kind == "box/cat").await;
            assert_eq!(mine.body, Body::of("mine"));
            let outer = r#"echo '{"kind":"cat","tense":"past","body":"not mine"}'
                echo '{"kind":"cat","tense":"future","body":"meow"}'; cat > /dev/null"#;
            want(&town, "town/join", json!({ "as": "box", "argv": ["sh", "-c", outer] }));
            let refused = until(&mut words, |w| w.kind == "town/error").await;
            assert_eq!(refused.body.value()["from"], "box");
            assert_eq!(refused.body.value()["heard"], "box/cat");
            // but it may want what the nearer one is: that is told to it
            let meow = |w: &Word| w.kind == "box/cat" && w.tense == Tense::Past;
            let meow = until(&mut words, move |w| meow(w) && w.body == Body::of("meow")).await;
            assert_eq!(meow.by, town.joins.borrow()["box/cat"].who.id, "said back by the cat");
            town.reopen();
        });
    }

    // a join at a kind another owns cannot say what is there: it is refused, ends as it begins, and
    // is answered as any join is as it ends, so one that joins again as it ends hears it
    #[test]
    fn a_join_at_a_kind_owned_ends() {
        run(async {
            let (_dir, town, mut words) = joining();
            let square = Resident { own: vec!["focus".into()], ..town.who("square", true) };
            let _square = town.come(&square).unwrap();
            want(&town, "town/join", json!({ "as": "focus", "argv": ["cat"], "about": "f" }));
            let refused = until(&mut words, |w| w.kind == "town/error").await;
            assert_eq!(refused.body.value()["heard"], "focus");
            assert!(!town.joins.borrow().contains_key("focus"), "it ends");
            let ended = until(&mut words, |w| w.kind == "town/join" && w.tense == Tense::Past);
            let ended = ended.await.body.value();
            assert_eq!((ended["code"].as_i64(), ended["about"].as_str()), (Some(-1), Some("f")));
        });
    }

    // a joined program that stops saying is still told, until it ends, as any program is
    #[test]
    fn a_join_that_stops_saying_is_still_told() {
        run(async {
            let (_dir, town, mut words) = joining();
            // its output closed, it says so aside, then says aside what it is told
            let program =
                r#"exec >&-; echo ready >&2; while read -r told; do echo "$told" >&2; done"#;
            want(&town, "town/join", json!({ "as": "mute", "argv": ["sh", "-c", program] }));
            until(&mut words, an_error("ready")).await;
            // long enough that its silence is heard first
            tokio::time::sleep(Duration::from_millis(100)).await;
            want(&town, "mute", json!("hi"));
            until(&mut words, an_error("hi")).await;
            town.reopen();
        });
    }

    // a join's kind is a kind: one ending in `/`, or with a part `..`, is refused
    #[test]
    fn a_join_is_as_a_kind() {
        run(async {
            let (_dir, town, mut words) = joining();
            for kind in ["box/", "a/../b"] {
                want(&town, "town/join", json!({ "as": kind, "argv": ["cat"] }));
                let e = until(&mut words, |w| w.kind == "town/error").await;
                assert!(e.body.value()["message"].as_str().unwrap().contains("is no kind"));
            }
            assert!(town.joins.borrow().is_empty());
        });
    }

    // a join that speaks a language says each unit of it as a word, and is told a future word in
    // it; what it cannot say is an error
    #[test]
    fn a_join_speaks_its_language() {
        run(async {
            let (dir, town, mut words) = joining();
            said(&town, &dir);
            want(&town, "town/join", json!({ "as": "cat", "argv": ["cat"], "speaks": "said" }));
            want(&town, "cat", json!({ "said": "hi" }));
            let back = until(&mut words, |w| w.kind == "cat" && w.tense == Tense::Past).await;
            assert_eq!(back.body, Parts::from(json!({ "said": "hi" })));
            want(&town, "cat", json!("no parts"));
            let e = until(&mut words, |w| w.kind == "town/error").await;
            assert_eq!(e.body.value()["message"], "cannot be said in its language");
            town.reopen();
        });
    }

    // a socket reached says it is made, hears and is told in its language, and as the other end
    // closes, the past `town/connect` says so; one that cannot be made says why
    #[test]
    fn a_socket_talks_until_it_closes() {
        run(async {
            let (dir, town, mut words) = joining();
            said(&town, &dir);
            let at = dir.join("socket");
            let listener = tokio::net::UnixListener::bind(&at).unwrap();
            // the other end: says hello, says back the first line it hears, and closes
            tokio::task::spawn_local(async move {
                use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
                let (conn, _) = listener.accept().await.unwrap();
                let (r, mut w) = conn.into_split();
                w.write_all(b"hello\n").await.unwrap();
                let mut line = String::new();
                tokio::io::BufReader::new(r).read_line(&mut line).await.unwrap();
                w.write_all(format!("back {line}").as_bytes()).await.unwrap();
            });
            want(
                &town,
                "town/connect",
                json!({ "as": "peer", "to": at, "speaks": "said", "about": "x" }),
            );
            let hello = until(&mut words, |w| w.kind == "peer").await;
            assert_eq!(hello.body, Parts::from(json!({ "said": "hello" })));
            want(&town, "peer", json!({ "said": "hi" }));
            let back = until(&mut words, |w| w.kind == "peer" && w.tense == Tense::Past).await;
            assert_eq!(back.body, Parts::from(json!({ "said": "back hi" })));
            let ended =
                until(&mut words, |w| w.kind == "town/connect" && w.tense == Tense::Past).await;
            assert_eq!(
                (ended.body.value()["about"].as_str(), ended.body.value()["code"].as_i64()),
                (Some("x"), Some(0))
            );
            assert!(
                town.may(&Word::new("peer", Tense::Past, Body::none()), &town.who("square", true))
                    .is_ok(),
                "let go as it ends"
            );
            want(&town, "town/connect", json!({ "as": "peer", "to": dir.join("none") }));
            let ended =
                until(&mut words, |w| w.kind == "town/connect" && w.tense == Tense::Past).await;
            assert!(ended.body.value()["out"].as_str().unwrap().starts_with("cannot connect to"));
        });
    }

    // a join that ends as it waits for its language, having said what is not JSON, ends: its
    // kind is let go, and the past `town/join` says how
    #[test]
    fn a_join_that_ends_as_it_waits_ends() {
        run(async {
            let (_dir, town, mut words) = joining();
            let argv = json!(["sh", "-c", "echo x; exit 2"]);
            want(&town, "town/join", json!({ "as": "gone", "argv": argv, "speaks": "unsaid" }));
            let ended =
                until(&mut words, |w| w.kind == "town/join" && w.tense == Tense::Past).await;
            assert_eq!(ended.body.value()["code"], 2);
            assert!(!town.joins.borrow().contains_key("gone"), "let go as it ends");
        });
    }

    // a join may say one language and hear another, `[says, hears]`
    #[test]
    fn a_join_says_one_language_and_hears_another() {
        run(async {
            let (dir, town, mut words) = joining();
            said(&town, &dir);
            // it hears JSON, and says what it hears in `said`, the line it was told
            want(
                &town,
                "town/join",
                json!({ "as": "two", "argv": ["cat"], "speaks": ["said", "json"] }),
            );
            want(&town, "two", json!("hi"));
            let told = |w: &Word| w.body.part("said").and_then(Parts::text).map(String::from);
            let hi = |w: &Word| told(w).is_some_and(|t| t.contains(r#""body":"hi""#));
            let back = until(&mut words, |w| w.kind == "two" && hi(w)).await;
            assert!(told(&back).unwrap().contains(r#""tense":"future""#), "told it in JSON");
            town.reopen();
        });
    }

    // what is wanted of town that is no verb, a typo or what town says, is an error from town
    #[test]
    fn town_has_only_its_verbs() {
        run(async {
            let (_dir, town, mut words) = joining();
            for kind in ["town/wrk", "town/residents/x", "town/x", "town/work/x"] {
                want(&town, kind, json!({}));
                let e = until(&mut words, |w| w.kind == "town/error").await;
                let verb = kind.strip_prefix("town/").unwrap();
                assert_eq!(e.body.value()["message"], format!("town has no `{verb}`"));
                assert_eq!((e.body.value()["from"].as_str(), e.by), (Some("town"), 0));
                assert_eq!(e.body.value()["heard"], kind);
            }
        });
    }

    // a wait of any number of ms comes back, a float from a Lua division too; one without a
    // number a clock can count, below 0 or too big, is an error, not silence or a panic
    #[test]
    fn a_wait_comes_back_or_says_why_not() {
        run(async {
            let (_dir, town, mut words) = joining();
            want(&town, "town/after", json!({ "ms": 250.0 / 10.0, "about": "x" }));
            want(&town, "town/after", json!({ "ms": -1 }));
            want(&town, "town/after", json!({ "ms": 1e300 }));
            // a last wait, of none: answered in order, so every refusal is said before it is back,
            // whichever of the two waits is back first
            want(&town, "town/after", json!({ "ms": 0, "about": "end" }));
            let after = |about: &'static str| {
                move |w: &Word| {
                    w.kind == "town/after"
                        && w.tense == Tense::Past
                        && w.body.value()["about"] == about
                }
            };
            let said = upto(&mut words, after("end")).await;
            let refused = said
                .iter()
                .filter(|w| w.kind == "town/error" && w.body.value()["heard"] == "town/after");
            if !said.iter().any(after("x")) {
                until(&mut words, after("x")).await;
            }
            assert_eq!(refused.count(), 2, "below 0, and too big to count");
        });
    }

    // a work that cannot start says why, as a join does, and its answer holds why, and all it
    // was asked with, `cmd` too
    #[test]
    fn a_work_that_cannot_start_says_why() {
        run(async {
            let dir = scratch();
            let world = [("PATH".into(), dir.as_os_str().into())].into_iter().collect();
            let town = Town::in_world(dir.log(), world);
            let mut words = town.words.listen();
            want(&town, "town/work", json!({ "cmd": "true", "about": "x" }));
            let why = "cannot run sh: sh is not on the world's PATH";
            let error = until(&mut words, an_error("cannot run")).await;
            assert_eq!(error.body.value()["message"], why);
            let past = |w: &Word| w.kind == "town/work" && w.tense == Tense::Past;
            let done = until(&mut words, past).await.body.value();
            assert_eq!(
                done,
                json!({ "cmd": "true", "about": "x", "code": -1, "out": why, "ms": done["ms"] })
            );
        });
    }

    // a town runs in its world: its `sh` is the world's, so a command goes where the world says,
    // here recorded, not run; its residents read the world's vars; and a join of what is not on
    // the world's PATH cannot start, and says so
    #[test]
    fn a_town_runs_in_its_world() {
        use std::os::unix::fs::PermissionsExt;
        run(async {
            let dir = scratch();
            let bin = dir.join("bin");
            std::fs::create_dir(&bin).unwrap();
            std::fs::write(bin.join("sh"), "#!/bin/sh\necho \"$@\" >> \"$HOME/ran\"\n").unwrap();
            let runs = std::fs::Permissions::from_mode(0o755);
            std::fs::set_permissions(bin.join("sh"), runs).unwrap();
            let world = [("HOME", dir.as_os_str()), ("PATH", bin.as_os_str())];
            let world = world.into_iter().map(|(k, v)| (k.into(), v.into())).collect();
            let town = Town::in_world(dir.log(), world);
            let mut words = town.words.listen();
            want(&town, "town/work", json!({ "cmd": "tmux kill-session", "about": "x" }));
            let past =
                |kind: &'static str| move |w: &Word| w.kind == kind && w.tense == Tense::Past;
            let done = until(&mut words, past("town/work")).await;
            assert_eq!(done.body.value()["code"], 0);
            let ran = std::fs::read_to_string(dir.join("ran")).unwrap();
            assert_eq!(ran, "-c tmux kill-session\n", "the world's sh, given the command");
            let lua = mlua::Lua::new();
            crate::lua::install(&lua, &town).unwrap();
            let home: String = lua.load("return os.getenv('HOME')").eval().unwrap();
            assert_eq!(home, dir.to_string_lossy(), "the world's HOME");
            assert!(lua
                .load("return os.getenv('USER')")
                .eval::<Option<String>>()
                .unwrap()
                .is_none());
            want(&town, "town/join", json!({ "as": "tmux", "argv": ["tmux", "-C"] }));
            let why = until(&mut words, an_error("cannot join")).await;
            let why = why.body.value()["message"].clone();
            assert_eq!(why, "cannot join tmux: tmux is not on the world's PATH");
            assert_eq!(until(&mut words, past("town/join")).await.body.value()["code"], -1);
        });
    }
}
