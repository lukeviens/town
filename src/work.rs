//! What town is asked to do: its own futures, each answered in the past with how it went and what
//! was sent with it. The README's *town*.
//!
//! - **`town/work`**: a program, `argv`, run to its end without keeping town waiting.
//! - **`town/join`**: talk with a program at a kind, until it ends.

use std::rc::Rc;

use tokio::io::{AsyncRead, AsyncWrite};

use crate::body::Body;
use crate::parts::Parts;
use crate::program::{Ends, Group};

use crate::town::{Joined, Listening, Resident, Town};
use crate::voice::{Life, Speaks, Voice};
use crate::word::{is_kind, Tense, Word, KIND};

/// What town is asked to do, by the verb of its want, `town/<verb>`: no other is.
pub(crate) const VERBS: [&str; 4] = ["work", "join", "open", "read"];

/// A want of town, `town/<verb>`, begun as it is said, so each is answered in the order asked. `by`
/// asked for it. A work outlives a reopen, as town's own; a read is answered by the door alone.
pub(crate) fn begin(town: &Rc<Town>, verb: &str, body: Body, by: u64) {
    let kind = format!("town/{verb}");
    // refused as the asker's own error, so one that asks as it answers an error is marked so
    let asker = town.resident(by);
    let fail = |why: String| town.error(&asker, Some(&kind), why);
    match verb {
        "work" => match argv(&body) {
            Ok(argv) => _ = tokio::task::spawn_local(done(town.clone(), argv, body)),
            Err(why) => fail(format!("a work needs {why}")),
        },
        "join" => {
            if let Err(why) = join(town, body, by) {
                fail(why);
            }
        }
        "open" => {
            let opening = town.opening.borrow().clone();
            if let Some(opening) = opening {
                opening.ask(town, &body);
            }
        }
        // a resident's want of another is refused at the door; this is town's own, said past it
        _ => fail(format!("town has no `{verb}`")),
    }
}

/// The program a work or a join runs, `argv`: its name, or its path, then what it is given, each
/// text, or a number as the text it is written as, `sleep 5`. Else what it needs.
fn argv(body: &Body) -> Result<Vec<String>, String> {
    let text = |p: &Parts| match p {
        Parts::Text(t) | Parts::Number(t) => Some(t.clone()),
        _ => None,
    };
    let argv = body.part("argv").and_then(Parts::list).ok_or("`argv`, the program to run")?;
    let argv = argv.iter().map(text).collect::<Option<Vec<_>>>();
    let argv = argv.ok_or("an `argv` of text and numbers")?;
    Some(argv).filter(|argv| !argv.is_empty()).ok_or("an `argv` that names a program".into())
}

/// Join a program as a resident at the kind `as`, owning it and what is under
/// it while it runs. It speaks `speaks`, or JSON; its kinds are paths from `as`: `a/b` is
/// `as/a/b`, and none is `as`. One a resident asks for, numbered `by`, is under its place, spoken
/// for by it, so it goes as that one goes. Joining a kind again ends the one before, as a reopen
/// does. Why it cannot begin, if it cannot.
fn join(town: &Rc<Town>, body: Body, by: u64) -> Result<(), String> {
    let kind = body.part("as").and_then(Parts::text).map(String::from);
    if let Some(kind) = kind.as_deref().filter(|k| !is_kind(k)) {
        return Err(format!("`{kind}` is no kind: {KIND}"));
    }
    let kind = kind.ok_or("a join needs `as`, the kind it stands at")?;
    let argv = argv(&body).map_err(|why| format!("a join needs {why}"))?;
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
    let refused = |e: String| {
        town.error(&who, Some(&kind), e.clone());
        let body = answered(body.clone(), |m| {
            m.set("code", -1i64);
            m.set("out", e);
        });
        town.talk(Word::new("town/join", Tense::Past, body));
        Ok(())
    };
    // a kind joined is its asker's: it may join there again, and none else may take it
    let before = town.joins.borrow().get(&kind).map(|j| (j.who.id, j.who.by));
    if before.is_some_and(|(_, by)| by != who.by) {
        return refused(format!("`{kind}` is joined by another"));
    }
    if let Err(e) = town.may_come(&who, before.map(|(id, _)| id)) {
        return refused(e);
    }
    // the one before is taken out, then let go, so this one owns its kind in its place, and what
    // its going says finds `joins` free to look at
    let before = town.joins.borrow_mut().remove(&kind);
    drop(before);
    let here = match town.come(&who) {
        Ok(here) => here,
        Err(e) => return refused(e),
    };
    // it hears from now what is wanted there, though it runs, and says its language, later
    let words = who.listening(town);
    let it = Join { town: town.clone(), resident: who.clone() };
    // among what runs for it, so it ends as it goes, and no longer keeps the town alive
    let run = tokio::task::spawn_local(it.run(argv, body, words));
    let ends = Ends(run.abort_handle());
    town.runs(who.id, run);
    town.joins
        .borrow_mut()
        .insert(kind, Joined { who, asked: Default::default(), _here: here, _run: ends });
    Ok(())
}

/// A kind joined to what it talks with.
struct Join {
    town: Rc<Town>,
    /// it, standing at its kind, in the language it speaks: waited for, if not said yet
    resident: Resident,
}

impl Join {
    /// From its start to its end. As it ends, its kind is let go, and its past `town/join` says
    /// how. One ended by a join again or a reopen says nothing: another stands in its place, and
    /// one that joined again as a join ends would join without end.
    async fn run(self, argv: Vec<String>, body: Body, words: Listening) {
        let (code, out) = self.ran(&argv, words).await;
        let (town, who) = (&self.town, &self.resident);
        let at = who.at.as_deref().unwrap_or_default();
        // its own, taken out, so its kind is let go: the run that ends is this one, which waits
        // for nothing more
        let mine = town.joins.borrow().get(at).is_some_and(|j| j.who.id == who.id);
        let joined = mine.then(|| town.joins.borrow_mut().remove(at));
        drop(joined);
        // one that cannot start says why, as any resident that fails does
        if let Some(why) = &out {
            town.error(who, Some("town/join"), why.clone());
        }
        let body = answered(body, |m| {
            m.set("code", i64::from(code));
            if let Some(out) = out {
                m.set("out", out);
            }
        });
        town.talk(Word::new("town/join", Tense::Past, body));
    }

    /// a program run, and talked with: how it ended
    async fn ran(&self, argv: &[String], words: Listening) -> (i32, Option<String>) {
        // a reopen, or a join again, ends the run, and the program with all it started; it lives
        // until it ends: one that stops saying is still told
        let who = &self.resident;
        let ran = Group::run(&self.town, who, argv, None, async |child, out, inp| {
            self.converse(out, inp, words, crate::program::ends(&self.town, child)).await;
            child.wait().await
        });
        match ran.await {
            Ok(status) => (status.ok().and_then(|s| s.code()).unwrap_or(-1), None),
            Err(e) => (-1, Some(format!("cannot join {}: {e}", argv[0]))),
        }
    }

    /// Both ways at once, as any resident lives (`live`), in its language once it is said, until
    /// the program ends.
    async fn converse(
        &self,
        out: impl AsyncRead + Unpin,
        mut inp: impl AsyncWrite + Unpin,
        words: Listening,
        end: impl std::future::Future,
    ) {
        let town = &self.town;
        let waits = async |m: &str| _ = town.notice(&self.resident, m.into());
        let (resident, voice) = (self.resident.clone(), Voice::new(out));
        let speaking = Life::speaking(town, resident, words, voice, waits);
        if let Some(resident) = speaking.await {
            resident.live(town, &mut inp, Some(end)).await;
        }
    }
}

/// The caller's body with what town has to say of it, if it has parts to say it among: the
/// caller's own fields ride back with the answer, so it can match on them (`on`'s `by`).
pub(crate) fn answered(body: Body, say: impl FnOnce(&mut Parts)) -> Body {
    match body.parts() {
        Parts::Map(_) => {
            let mut parts = body.parts().clone();
            say(&mut parts);
            Body::of(parts)
        }
        _ => body,
    }
}

/// What town keeps of `kind` in `tense`, said back with what was asked: the tense is when, as town
/// keeps only facts. Present, the fact now; past, the facts before it, oldest first, as far as the
/// log's tail; of a pattern (`kind/`, `/kind`, `/`), by kind. The future is not kept: it is heard.
/// Only what the asker `hears`: a kind it does not is refused, and a pattern keeps only those it
/// does.
pub(crate) fn read(town: &Town, body: &Body, hears: impl Fn(&str) -> bool) -> Result<Body, String> {
    let kind = body.part("kind").and_then(Parts::text).ok_or("a read names a `kind`")?;
    let tense = body.part("tense").and_then(Parts::text).unwrap_or("present");
    let one = is_kind(kind);
    if one && !hears(kind) {
        return Err(format!("`{kind}`: it reads only what it hears"));
    }
    let read = match tense {
        "present" => {
            let mut facts = town.facts(kind);
            facts.retain(|k, _| hears(k));
            match one {
                true => facts.into_values().next().map_or(Parts::None, |b| b.parts().clone()),
                false => {
                    Parts::Map(facts.into_iter().map(|(k, b)| (k, b.parts().clone())).collect())
                }
            }
        }
        "past" => {
            let list =
                |bodies: Vec<Body>| Parts::List(bodies.iter().map(|b| b.parts().clone()).collect());
            let mut past = town.past(kind);
            past.retain(|k, _| hears(k));
            match one {
                true => list(past.into_values().next().unwrap_or_default()),
                false => Parts::Map(past.into_iter().map(|(k, b)| (k, list(b))).collect()),
            }
        }
        "future" => return Err("the future is not kept: it is heard, by `kinds`".into()),
        _ => return Err("a read's tense is present or past".into()),
    };
    Ok(answered(body.clone(), |m| m.set("body", read)))
}

/// A program run to its end, as `argv` names it, in a group of its own as any program is: the past
/// `town/work` carries all it was asked with, `argv` too, and what it said, how it ended, by its
/// `code` or the `signal` that ended it, and its ms. A wait is one, `sleep`; a shell is one,
/// `sh -c`. Each line on its stderr is a `town/error` from it. One that cannot start says why, as a
/// `town/error` and in `out`, as a join does.
async fn done(town: Rc<Town>, argv: Vec<String>, body: Body) {
    use std::os::unix::process::ExitStatusExt;
    use tokio::io::AsyncReadExt;
    let began = town.moment();
    let who = town.who(&argv[0], false);
    // what it says, read as it says it, so it never waits to be read; then, once all it started
    // has ended with it, what is left to read, and no more: one that left its group may hold its
    // output open for ever, and is not waited for
    let ran = Group::run(&town, &who, &argv, None, async |child, mut out, inp| {
        drop(inp);
        let (mut said, mut buf) = (Vec::new(), vec![0; 1 << 16]);
        let status = loop {
            tokio::select! {
                status = child.wait() => break status,
                n = out.read(&mut buf) => match n {
                    Ok(0) | Err(_) => break child.wait().await,
                    Ok(n) => said.extend_from_slice(&buf[..n]),
                },
            }
        };
        (status, said, out)
    })
    .await;
    let (code, signal, out) = match ran {
        Ok((status, mut said, out)) => {
            left(&out, &mut said);
            let status = status.ok();
            let code = status.and_then(|s| s.code()).unwrap_or(-1);
            // as a shell's `$(…)` gives it: its last newlines let go, all else as it was said
            let out = String::from_utf8_lossy(&said).trim_end_matches('\n').to_string();
            (code, status.and_then(|s| s.signal()), out)
        }
        Err(e) => {
            let why = format!("cannot run {}: {e}", argv[0]);
            town.error(&Resident::town(), Some("town/work"), why.clone());
            (-1, None, why)
        }
    };
    // by town's clock, as `when` is: a test's counts, so its ms is the same each run
    let ms = town.moment().saturating_sub(began);
    let body = answered(body, |m| {
        m.set("code", i64::from(code));
        if let Some(signal) = signal {
            m.set("signal", i64::from(signal));
        }
        m.set("out", out);
        m.set("ms", ms);
    });
    town.talk(Word::new("town/work", Tense::Past, body));
}

/// What is left to read of `out` now, without waiting: what was said before its sayers ended. Read
/// through a copy of its descriptor, which shares its not waiting, so a read finds what is there,
/// or that nothing is.
fn left(out: &tokio::process::ChildStdout, said: &mut Vec<u8>) {
    use std::io::Read;
    use std::os::fd::AsFd;
    let Ok(fd) = out.as_fd().try_clone_to_owned() else { return };
    let mut out = std::fs::File::from(fd);
    let mut buf = vec![0u8; 1 << 16];
    while let Ok(n @ 1..) = out.read(&mut buf) {
        said.extend_from_slice(&buf[..n]);
    }
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

    // a kind joined is its asker's: another that asks to join there is refused, and the first
    // stays; its asker may join there again
    #[test]
    fn a_join_is_its_askers_alone() {
        run(async {
            let (_dir, town, mut words) = joining();
            let a = Resident { kinds: vec!["x".into()], ..town.who("a", false) };
            let b = Resident { kinds: vec!["x".into()], ..town.who("b", false) };
            let (_a, _b) = (town.come(&a).unwrap(), town.come(&b).unwrap());
            let ask = |who: &Resident| {
                let body = Body::of(json!({ "as": "cat", "argv": ["cat"] }));
                town.said(Word::new("town/join", Tense::Future, body), who);
            };
            ask(&a);
            let first = town.joins.borrow()["cat"].who.id;
            ask(&b);
            let refused = until(&mut words, |w| w.kind == "town/join" && w.tense == Tense::Past);
            assert!(refused.await.body.value()["out"].as_str().unwrap().contains("by another"));
            assert_eq!(town.joins.borrow()["cat"].who.id, first, "the first stays");
            ask(&a);
            assert_ne!(town.joins.borrow()["cat"].who.id, first, "its asker may join again");
            town.reopen();
        });
    }

    /// what is wanted of a kind, as a resident asks it
    fn want(town: &Town, kind: &str, body: Value) {
        town.talk(Word::new(kind, Tense::Future, Body::of(body)));
    }

    /// `line`, town's own language for a join that says lines, here as a town opens it
    async fn line(town: &Rc<Town>) {
        wired(town, &crate::residents::own()[0].join("languages/line.lua")).await;
    }

    /// `said`, a language as `line` is, but that a line read is the body `{ said }`
    async fn said(town: &Rc<Town>, dir: &std::path::Path) {
        let tense = r#"{ tense = choice{ be("", "past"), be("", "future") } }"#;
        let body = r#"{ body = seq{ { said = upto("\n") } } }"#;
        speaking(town, dir, "said", &format!("seq{{ {tense}, {body} }}")).await;
    }

    // a joined program owns its kind while it runs and talks both ways; as it ends, the past
    // `town/join` says how, with what was sent. Joined again, the one before ends and says nothing.
    // One that cannot start says so as it ends; a number in argv is its text; and a join with
    // nothing to run, an argv of anything else, or as one of town's own words, is an error
    #[test]
    fn a_join_talks_until_it_ends() {
        run(async {
            let (_dir, town, mut words) = joining();
            line(&town).await;
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
            assert_eq!(bad.body.value()["message"], "a join needs an `argv` of text and numbers");
            want(&town, "town/join", json!({ "as": "town/work", "argv": ["cat"] }));
            let bad = until(&mut words, |w| w.kind == "town/error").await;
            assert!(bad.body.value()["message"].as_str().unwrap().contains("town's own"));
            town.reopen();
        });
    }

    // a join is told the facts at its kind, by their paths from it, and none beside it, then that
    // it is told first `now`, not what is at its kind, which it says itself; what it says is said
    // at its kind, and what names no kind is refused; it is told the future words there, and its
    // stderr is heard, in its own time
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
            let program = r#"read -r joined
                printf '{"kind":"../out","tense":"past","body":null}\n'
                [ "$joined" = '{"kind":"","tense":"past","body":null}' ] &&
                  printf '{"kind":"joined","tense":"past","body":"yes"}\n'
                printf '{"kind":"again","tense":"future","body":null}\n'
                echo "said aside" >&2
                while read -r told; do printf '%s\n' "$told" | sed 's/"future"/"past"/'; done"#;
            want(&town, "town/join", json!({ "as": "box", "argv": ["sh", "-c", program] }));
            let said = |kind: &'static str| move |w: &Word| w.kind == kind;
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

    // a join is told what is wanted of it as it speaks, not before: one that says much before it
    // reads, what waits for it more than a pipe contains, is heard and told both
    #[test]
    fn a_long_greeting_does_not_stop_a_join() {
        run(async {
            let (_dir, town, _) = joining();
            let long = "x".repeat(200);
            let mut words = town.words.listen();
            // 2000 lines said before one is read; then what it is told read, `now` and 2000 wants
            let program = r#"i=0; while [ $i -lt 2000 ]; do
                  echo '{"kind":"said","tense":"past","body":null}'; i=$((i+1)); done
                echo '{"kind":"done","tense":"past","body":null}'
                head -n 2001 > /dev/null; echo '{"kind":"told","tense":"past","body":null}'"#;
            want(&town, "town/join", json!({ "as": "big", "argv": ["sh", "-c", program] }));
            for n in 0..2000 {
                want(&town, &format!("big/{n}"), json!(long));
            }
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
            line(&town).await; // said after: it waited for it
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
            said(&town, &dir).await;
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
            said(&town, &dir).await;
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

    // a work that cannot start says why, as a join does, and its answer says why, and all it
    // was asked with, `argv` too
    #[test]
    fn a_work_that_cannot_start_says_why() {
        run(async {
            let dir = scratch();
            let world = [("PATH".into(), dir.as_os_str().into())].into_iter().collect();
            let town = Town::in_world(dir.log(), world);
            let mut words = town.words.listen();
            want(&town, "town/work", json!({ "argv": ["true"], "about": "x" }));
            let why = "cannot run true: true is not on the world's PATH";
            let error = until(&mut words, an_error("cannot run")).await;
            assert_eq!(error.body.value()["message"], why);
            let past = |w: &Word| w.kind == "town/work" && w.tense == Tense::Past;
            let done = until(&mut words, past).await.body.value();
            // its ms by the test's clock: begun, its error stamped, ended, two counts
            assert_eq!(
                done,
                json!({ "argv": ["true"], "about": "x", "code": -1, "out": why, "ms": 2 })
            );
        });
    }

    // a work ends as what it started ends: one that left its group, and holds its output open, is
    // not waited for, and what was said before is kept
    #[test]
    fn a_work_ends_though_one_it_started_left() {
        // the one that left, ended however the test ends
        struct Left(std::path::PathBuf);
        impl Drop for Left {
            fn drop(&mut self) {
                if let Some(pid) =
                    std::fs::read_to_string(&self.0).ok().and_then(|p| p.trim().parse().ok())
                {
                    crate::unix::end_group(pid);
                }
            }
        }
        run(async {
            let (dir, town, mut words) = joining();
            let _left = Left(dir.join("pid"));
            // `set -m`: what it starts is a job, in a group of its own, out of the work's
            let left =
                format!("set -m; sleep 30 & echo $! > {}; echo said", dir.join("pid").display());
            want(&town, "town/work", json!({ "argv": ["sh", "-c", left] }));
            let past = |w: &Word| w.kind == "town/work" && w.tense == Tense::Past;
            let done = until(&mut words, past).await.body.value();
            assert_eq!(done["out"], "said");
        });
    }

    // a town runs in its world: a work runs the program the world's PATH finds, here one that
    // records what it is given; its residents read the world's vars; and a join of what is not on
    // the world's PATH cannot start, and says so
    #[test]
    fn a_town_runs_in_its_world() {
        use std::os::unix::fs::PermissionsExt;
        run(async {
            let dir = scratch();
            let bin = dir.join("bin");
            std::fs::create_dir(&bin).unwrap();
            let notes = "#!/bin/sh\necho \"$@\" >> \"$HOME/ran\"\n";
            std::fs::write(bin.join("notes"), notes).unwrap();
            let runs = std::fs::Permissions::from_mode(0o755);
            std::fs::set_permissions(bin.join("notes"), runs).unwrap();
            let world = [("HOME", dir.as_os_str()), ("PATH", bin.as_os_str())];
            let world = world.into_iter().map(|(k, v)| (k.into(), v.into())).collect();
            let town = Town::in_world(dir.log(), world);
            let mut words = town.words.listen();
            want(&town, "town/work", json!({ "argv": ["notes", "kill-session"], "about": "x" }));
            let past =
                |kind: &'static str| move |w: &Word| w.kind == kind && w.tense == Tense::Past;
            let done = until(&mut words, past("town/work")).await;
            assert_eq!(done.body.value()["code"], 0);
            let ran = std::fs::read_to_string(dir.join("ran")).unwrap();
            assert_eq!(ran, "kill-session\n", "the world's, given what it was asked");
            // a Lua file, as `lua`, run in the world, reads it
            let env = dir.join("env.lua");
            let reads =
                r#"talk(event("env", { home = os.getenv("HOME"), user = os.getenv("USER") }))"#;
            std::fs::write(&env, format!("{reads}; return {{ own = {{ \"env\" }} }}")).unwrap();
            wired(&town, &env).await;
            let read = until(&mut words, |w| w.kind == "env").await.body.value();
            assert_eq!(read, json!({ "home": dir.to_string_lossy() }), "the world's HOME alone");
            want(&town, "town/join", json!({ "as": "tmux", "argv": ["tmux", "-C"] }));
            let why = until(&mut words, an_error("cannot join")).await;
            let why = why.body.value()["message"].clone();
            assert_eq!(why, "cannot join tmux: tmux is not on the world's PATH");
            assert_eq!(until(&mut words, past("town/join")).await.body.value()["code"], -1);
        });
    }

    // a read is said back with what town keeps: the fact now, or the facts before it, oldest
    // first, by kind for a pattern; the future is not kept; and one at the square may ask, of any
    // kind. The ask is town's alone, and the answer the asker's: no other hears either, and the
    // log keeps none
    #[test]
    fn a_read_is_what_town_keeps() {
        let (dir, town, mut words) = joining();
        let say = |kind: &str, n: i64| town.talk(Word::new(kind, Tense::Present, Body::of(n)));
        say("box/a", 1);
        say("box/a", 2);
        say("box/b", 3);
        say("other", 4);
        let asker = town.who("sq", true);
        let mut mine = asker.listening(&town);
        let other = Resident { kinds: vec!["/town/read".into()], ..town.who("other", true) };
        let mut others = other.listening(&town);
        let mut read = |kind: &str, tense: &str| {
            let asked = json!({ "kind": kind, "tense": tense, "id": "q" });
            town.said(Word::new("town/read", Tense::Future, Body::of(asked)), &asker);
            let answer = |w: &Word| w.kind == "town/read" && w.tense == Tense::Past;
            let w = std::iter::from_fn(|| mine.try_recv().ok()).find(answer);
            let w = w.or_else(|| {
                std::iter::from_fn(|| words.try_recv().ok()).find(|w| w.kind == crate::word::ERROR)
            });
            let w = w.expect("answered");
            (w.kind.clone(), w.body.value())
        };
        let (kind, now) = read("box/a", "present");
        assert_eq!(
            (kind.as_str(), &now["body"], &now["id"]),
            ("town/read", &json!(2), &json!("q"))
        );
        assert_eq!(read("box/none", "present").1["body"], Value::Null, "none, where there is none");
        assert_eq!(read("box/", "present").1["body"], json!({ "box/a": 2, "box/b": 3 }));
        assert_eq!(read("box/a", "past").1["body"], json!([1, 2]), "oldest first");
        assert_eq!(read("box/", "past").1["body"], json!({ "box/a": [1, 2], "box/b": [3] }));
        assert_eq!(read("other", "present").1["body"], json!(4), "what it does not hear, too");
        let (kind, refused) = read("box/a", "future");
        assert_eq!(kind, "town/error");
        assert!(refused["message"].as_str().unwrap().contains("not kept"));
        let heard = |w: &Word| w.kind == "town/read";
        assert!(!std::iter::from_fn(|| others.try_recv().ok()).any(|w| heard(&w)), "another");
        assert!(!std::iter::from_fn(|| words.try_recv().ok()).any(|w| heard(&w)), "nor all");
        let logged = std::fs::read_to_string(dir.log()).unwrap();
        assert!(!logged.contains("town/read"), "the log keeps none");
    }

    // a town that comes by a name reads only what its name gives: a kind it does not hear is
    // refused, and a pattern keeps only those it does
    #[test]
    fn a_visitor_reads_only_its_name() {
        let dir = scratch();
        let town = Town::with_log(dir.log());
        let say = |kind: &str, n: i64| town.talk(Word::new(kind, Tense::Present, Body::of(n)));
        say("box/a", 1);
        say("other", 2);
        let visitor = Resident { kinds: vec!["box/".into()], visitor: true, ..town.who("v", true) };
        let mut told = visitor.listening(&town);
        let mut read = |kind: &str| {
            let asked = json!({ "kind": kind, "tense": "present" });
            town.said(Word::new("town/read", Tense::Future, Body::of(asked)), &visitor);
            let read = |w: &Word| w.kind == "town/read" || w.kind == crate::word::ERROR;
            std::iter::from_fn(|| told.try_recv().ok()).find(read).expect("answered")
        };
        assert_eq!(read("/").body.value()["body"], json!({ "box/a": 1 }), "what it hears");
        let refused = read("other");
        assert_eq!(refused.kind, crate::word::ERROR, "not what it does not");
        assert!(refused.body.value()["message"].as_str().unwrap().contains("reads only"));
    }
}
