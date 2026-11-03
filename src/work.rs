//! work — town causing the world to change. A future `work` runs outside without holding up
//! town; the past `work` carries what it cost and whatever the caller sent with it. A future
//! `after` is town keeping time: once its `ms` have gone, the past `after` carries what was sent. A
//! future `join` is a program town keeps talking to until it ends, and a future `connect` a socket:
//! then the past `join` or `connect` carries how it ended, and what was sent.

use std::rc::Rc;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::mpsc;

use crate::body::Body;
use crate::connection::Heard;
use crate::parts::Parts;
use crate::town::{Joined, Town};
use crate::word::{Tense, Word};

/// Answer every future `work`, `after`, `join` and `connect` with a past word of its kind. Town's
/// own, not a resident's: it outlives a reopen.
pub fn answer(town: &Rc<Town>) {
    // hearing from now, not from when the task first runs: what residents say as they open, before
    // then, is answered too
    let mut words = town.words.subscribe();
    let town = town.clone();
    tokio::task::spawn_local(async move {
        loop {
            let w = match words.recv().await {
                Ok(w) => w,
                Err(RecvError::Lagged(_)) => continue,
                Err(RecvError::Closed) => break,
            };
            if w.tense != Tense::Future {
                continue;
            }
            // ms as any number: Lua's `/` makes a float of every division. A clock takes none below
            // 0, and none too big to count
            if w.kind == "after" {
                let ms = w.body.part("ms").and_then(Parts::number);
                match ms.and_then(|ms| Duration::try_from_secs_f64(ms / 1000.0).ok()) {
                    Some(wait) => {
                        tokio::task::spawn_local(waited(town.clone(), wait, w.body.clone()));
                    }
                    None => town.error("after", Some("after"), "a wait needs ms: 0 or more".into()),
                }
                continue;
            }
            if w.kind == "join" || w.kind == "connect" {
                begin(&town, if w.kind == "join" { "join" } else { "connect" }, w.body);
                continue;
            }
            // a future word of a joined kind is what its program is to be told
            if let Some(j) = town.joins.borrow().get(&w.kind) {
                match told(&w.body, j, &town) {
                    Ok(Some(bytes)) => drop(j.to.send(bytes)),
                    Ok(None) => {}
                    Err(e) => town.error(&w.kind, Some(&w.kind), e),
                }
                continue;
            }
            if w.kind != "work" {
                continue;
            }
            let Some(cmd) = w.body.part("cmd").and_then(Parts::text).map(String::from) else {
                continue;
            };
            tokio::task::spawn_local(done(town.clone(), cmd, w.body.clone()));
        }
    });
}

/// What a kind is joined to: a program it runs, or a socket it reaches, a path or a host:port.
enum Reach {
    Run(Vec<String>),
    At(String),
}

/// Join a program as a kind, or connect to a socket as one: each line it says is a past word of
/// that kind, and each future word of that kind is a line written to it; or, if it speaks a
/// language, as `speaks` names it, what it says in it, and what it is told. Joining a kind again
/// ends the one before. What a kind is joined to is what it says, so only town says that kind while
/// it runs. A reopen ends it: a resident joins again as it opens.
fn begin(town: &Rc<Town>, verb: &'static str, body: Body) {
    let kind = body.part("as").and_then(Parts::text).map(String::from);
    let reach = match verb {
        // a number in it is the text it is written as: `sleep 5`
        "join" => match body.part("argv").and_then(Parts::list) {
            Some(l) => match l
                .iter()
                .map(|p| match p {
                    Parts::Text(t) | Parts::Number(t) => Some(t.clone()),
                    _ => None,
                })
                .collect::<Option<Vec<_>>>()
            {
                Some(argv) => Some(argv).filter(|argv| !argv.is_empty()).map(Reach::Run),
                None => {
                    let why = "an `argv` is text and numbers".into();
                    return town.error(verb, Some(verb), why);
                }
            },
            None => None,
        },
        _ => body.part("to").and_then(Parts::text).map(|to| Reach::At(to.into())),
    };
    let (Some(kind), Some(reach)) = (kind, reach) else {
        let needs = if verb == "join" { "`argv` to run" } else { "`to`, a path or a host:port" };
        return town.error(verb, Some(verb), format!("a {verb} needs `as`, and {needs}"));
    };
    // town's own words mean what town makes them mean: a program cannot speak as one
    if Town::machinery().contains_key(&kind) {
        return town.error(verb, Some(verb), format!("`{kind}` is town's own, not to {verb}"));
    }
    let speaks = match body.part("speaks").map(|f| f.text().ok_or("`speaks` names a language")) {
        None => None,
        Some(Ok(name)) => Some(name.to_string()),
        Some(Err(e)) => return town.error(verb, Some(verb), e.into()),
    };
    let (tx, rx) = mpsc::unbounded_channel();
    let talk =
        Talk { town: town.clone(), kind: kind.clone(), speaks: speaks.clone(), me: tx.clone() };
    let run = tokio::task::spawn_local(talk.run(verb, reach, rx, body));
    let j = Joined { to: tx, run: run.abort_handle(), speaks };
    match town.joins.borrow_mut().insert(kind.clone(), j) {
        Some(before) => before.run.abort(),
        None => town.hold(&kind),
    }
    let mut tasks = town.tasks.borrow_mut();
    tasks.retain(|t| !t.is_finished()); // a resident may join again and again: keep only what runs
    tasks.push(run);
}

/// A kind joined to what it talks with.
struct Talk {
    town: Rc<Town>,
    kind: String,
    /// the language it speaks, by name: waited for, if it is not said yet, and read and written as
    /// it is now, so both ways are one language
    speaks: Option<String>,
    me: mpsc::UnboundedSender<Vec<u8>>,
}

impl Talk {
    /// From its start to its end. It is said as a bodiless word as it starts, since whatever was
    /// said to one before it, a subscription, a mode, is gone with that one. As it ends, its kind
    /// is let go, if it was not joined again, and the past `join` or `connect` says how it ended.
    async fn run(
        self,
        verb: &str,
        reach: Reach,
        mut told: mpsc::UnboundedReceiver<Vec<u8>>,
        body: Body,
    ) {
        if let Some(name) = &self.speaks {
            if self.town.language(name, &self.kind).await.is_none() {
                return;
            }
        }
        let (code, out) = match reach {
            Reach::Run(argv) => self.ran(&argv, &mut told).await,
            Reach::At(to) => match self.reached(&to, &mut told).await {
                Ok(()) => (0, None),
                Err(e) => (-1, Some(format!("cannot connect to {to}: {e}"))),
            },
        };
        let town = &self.town;
        let mine = town.joins.borrow().get(&self.kind).is_some_and(|j| j.to.same_channel(&self.me));
        if mine {
            town.joins.borrow_mut().remove(&self.kind);
            town.free(&self.kind);
        }
        let body = answered(body, |m| {
            m.set("code", i64::from(code));
            if let Some(out) = out {
                m.set("out", out);
            }
        });
        town.talk(Word::new(verb, Tense::Past, body));
    }

    /// a program run, and talked with: how it ended
    async fn ran(
        &self,
        argv: &[String],
        told: &mut mpsc::UnboundedReceiver<Vec<u8>>,
    ) -> (i32, Option<String>) {
        // a reopen, or a join again, ends the run, and the program goes with it, and all it
        // started: a group of its own, as a resident's is
        let child = tokio::process::Command::new(&argv[0])
            .args(&argv[1..])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .process_group(0)
            .kill_on_drop(true)
            .spawn();
        let mut child = match child {
            Err(e) => return (-1, Some(format!("cannot join {}: {e}", argv[0]))),
            Ok(child) => child,
        };
        let _group = crate::residents::Group::of(&self.town, &child);
        if let (Some(out), Some(inp)) = (child.stdout.take(), child.stdin.take()) {
            self.converse(out, inp, told).await;
        }
        // what it says has ended; so, or soon, has it
        let status = child.wait().await;
        (status.ok().and_then(|s| s.code()).unwrap_or(-1), None)
    }

    /// a socket reached, and talked with, until it closes: a path, or a host:port
    async fn reached(
        &self,
        to: &str,
        told: &mut mpsc::UnboundedReceiver<Vec<u8>>,
    ) -> std::io::Result<()> {
        if to.contains('/') {
            let (r, w) = tokio::net::UnixStream::connect(to).await?.into_split();
            self.converse(r, w, told).await;
        } else {
            let (r, w) = tokio::net::TcpStream::connect(to).await?.into_split();
            self.converse(r, w, told).await;
        }
        Ok(())
    }

    /// Both ways, until it hangs up: each line it says, or each unit of its language, is a past
    /// word of the kind, and what it is told is written to it.
    async fn converse(
        &self,
        out: impl AsyncRead + Unpin,
        mut inp: impl AsyncWrite + Unpin,
        told: &mut mpsc::UnboundedReceiver<Vec<u8>>,
    ) {
        let (town, kind) = (&self.town, self.kind.as_str());
        town.talk(Word::new(kind, Tense::Past, Body::none()));
        let mut heard = Heard::new(out);
        loop {
            tokio::select! {
                said = said(&mut heard, &self.speaks, town) => match said {
                    Some(Ok(body)) => town.talk(Word::new(kind, Tense::Past, body)),
                    Some(Err((bytes, name))) => {
                        let said = String::from_utf8_lossy(&bytes);
                        town.misheard(kind, said.trim_end_matches('\n'), format!("not {name}"));
                    }
                    None => return,
                },
                Some(bytes) = told.recv() => {
                    if inp.write_all(&bytes).await.is_err() {
                        return;
                    }
                }
            }
        }
    }
}

/// What is said next: a line, as text, or a unit of its language, as its parts; or the bytes that
/// are not of it.
async fn said<R: AsyncRead + Unpin>(
    heard: &mut Heard<R>,
    speaks: &Option<String>,
    town: &Town,
) -> Option<Result<Body, (Vec<u8>, String)>> {
    match speaks {
        None => heard.next().await.map(|line| Ok(Body::of(line))),
        // gone only as the town opens again, which ends what it speaks with
        Some(name) => match heard.unit(&*town.languages.get(name).ok()?, &town.languages).await? {
            Ok(parts) => Some(Ok(Body::of(parts))),
            Err(bytes) => Some(Err((bytes, name.clone()))),
        },
    }
}

/// What a future word of a joined kind tells its program: in its language, if it speaks one; else a
/// line, text as it is and anything else as JSON. A bodiless one tells it nothing.
fn told(body: &Body, j: &Joined, town: &Town) -> Result<Option<Vec<u8>>, String> {
    if body.is_none() {
        return Ok(None);
    }
    match &j.speaks {
        Some(name) => match town.languages.bytes(name, body.parts()) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(e) => Err(e),
        },
        None => {
            let mut line = match body.parts() {
                Parts::Text(t) => t.clone().into_bytes(),
                _ => body.json().into_owned(),
            };
            line.push(b'\n');
            Ok(Some(line))
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

/// The time asked for, gone: said back as a past `after`, with everything the caller sent.
async fn waited(town: Rc<Town>, wait: Duration, body: Body) {
    tokio::time::sleep(wait).await;
    town.talk(Word::new("after", Tense::Past, body));
}

/// A command run in a shell: the past `work` carries what it said, how it ended, and what it
/// cost.
async fn done(town: Rc<Town>, cmd: String, body: Body) {
    let began = std::time::Instant::now();
    let out = tokio::process::Command::new("/bin/sh").arg("-c").arg(&cmd).output().await;
    let ms = began.elapsed().as_millis() as u64;
    let (code, stdout) = match &out {
        Ok(o) => (
            o.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&o.stdout).trim_end().to_string(),
        ),
        Err(_) => (-1, String::new()),
    };
    let body = answered(body, |m| {
        m.remove("cmd");
        m.set("code", i64::from(code));
        m.set("out", stdout);
        m.set("ms", ms);
    });
    town.talk(Word::new("work", Tense::Past, body));
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    use crate::testing::until;

    // a joined program says it started, talks both ways, and as it ends the past `join` says how,
    // with what was sent; joined again, the one before ends; one that cannot start says so as it
    // ends, and a join with nothing to run, or as one of town's own words, is an error
    #[test]
    fn a_join_talks_until_it_ends() {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        tokio::task::LocalSet::new().block_on(&rt, async {
            let log = std::env::temp_dir().join(format!("town-join-{}.log", std::process::id()));
            let town = Town::with_log(log.clone());
            answer(&town);
            let mut words = town.words.subscribe();
            tokio::task::yield_now().await;
            let join = |argv: Value, about: &str| {
                Word::new(
                    "join",
                    Tense::Future,
                    Body::of(json!({ "as": "cat", "argv": argv, "about": about })),
                )
            };
            town.talk(join(json!(["cat"]), "first"));
            until(&mut words, |w| w.kind == "cat" && w.body.is_none()).await;
            assert!(
                !town.admits(&Word::new("cat", Tense::Past, Body::none())),
                "town's, while joined"
            );
            town.talk(Word::new("cat", Tense::Future, Body::of(json!("hi"))));
            until(&mut words, |w| w.kind == "cat" && w.body == "hi").await;
            // joined again: the first ends, and only the second answers
            town.talk(join(json!(["sh", "-c", "read -r l; echo got $l; exit 3"]), "second"));
            until(&mut words, |w| w.kind == "cat" && w.body.is_none()).await;
            town.talk(Word::new("cat", Tense::Future, Body::of(json!("x"))));
            until(&mut words, |w| w.kind == "cat" && w.body == "got x").await;
            let ended = until(&mut words, |w| w.kind == "join" && w.tense == Tense::Past).await;
            assert_eq!(
                (ended.body.value()["about"].as_str(), ended.body.value()["code"].as_i64()),
                (Some("second"), Some(3))
            );
            assert!(town.admits(&Word::new("cat", Tense::Past, Body::none())), "let go as it ends");
            town.talk(join(json!(["/no/such/program"]), "none"));
            let none = until(&mut words, |w| w.kind == "join" && w.tense == Tense::Past).await;
            assert_eq!(none.body.value()["code"], -1);
            assert!(none.body.value()["out"].as_str().unwrap().contains("/no/such/program"));
            town.talk(join(json!([]), "empty"));
            let bad = until(&mut words, |w| w.kind == "error").await;
            assert_eq!(bad.body.value()["heard"], "join");
            // a number in argv is its text; anything else in it is no argv
            town.talk(join(json!(["sh", "-c", "exit $0", 4]), "numbered"));
            let ended = until(&mut words, |w| w.kind == "join" && w.tense == Tense::Past).await;
            assert_eq!(ended.body.value()["code"], 4);
            town.talk(join(json!(["sh", {}]), "odd"));
            let bad = until(&mut words, |w| w.kind == "error").await;
            assert_eq!(bad.body.value()["message"], "an `argv` is text and numbers");
            // nor as one of town's own words
            let work = json!({ "as": "work", "argv": ["cat"] });
            town.talk(Word::new("join", Tense::Future, Body::of(work)));
            let bad = until(&mut words, |w| w.kind == "error").await;
            assert!(bad.body.value()["message"].as_str().unwrap().contains("town's own"));
            town.tasks.borrow_mut().drain(..).for_each(|h| h.abort());
            std::fs::remove_file(&log).ok();
        });
    }

    // a join that speaks a language says each unit of it as a past word's parts, and is told a
    // future word's parts in it; what it cannot say is an error, and so is a language there is
    // none of
    #[test]
    fn a_join_speaks_its_language() {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        tokio::task::LocalSet::new().block_on(&rt, async {
            let log = std::env::temp_dir().join(format!("town-joinas-{}.log", std::process::id()));
            let town = Town::with_log(log.clone());
            let line = Parts::from(json!({ "part": ["said", { "upto": "\n" }] }));
            town.languages.say("line", crate::language::Grammar::of(&line).unwrap());
            answer(&town);
            let mut words = town.words.subscribe();
            tokio::task::yield_now().await;
            let join = json!({ "as": "cat", "argv": ["cat"], "speaks": "line" });
            town.talk(Word::new("join", Tense::Future, Body::of(join)));
            until(&mut words, |w| w.kind == "cat" && w.body.is_none()).await;
            let said = Body::of(json!({ "said": "hi" }));
            town.talk(Word::new("cat", Tense::Future, said));
            let back = until(&mut words, |w| {
                w.kind == "cat" && w.tense == Tense::Past && !w.body.is_none()
            });
            assert_eq!(back.await.body, Parts::from(json!({ "said": "hi" })));
            town.talk(Word::new("cat", Tense::Future, Body::of("no parts")));
            let e = until(&mut words, |w| w.kind == "error").await;
            assert_eq!(e.body.value()["message"], "cannot be written as line");
            let none = json!({ "as": "cat", "argv": ["cat"], "speaks": "nope" });
            town.talk(Word::new("join", Tense::Future, Body::of(none)));
            let e = until(&mut words, |w| w.kind == "error").await;
            let waits = e.body.value()["message"].as_str().unwrap().to_string();
            assert_eq!(waits, "no language is called nope yet: it waits for one");
            // said at last, it is spoken: the join starts
            let nope = json!({ "name": "nope", "grammar": { "upto": "\n" } });
            town.talk(Word::new("language", Tense::Past, Body::of(nope)));
            until(&mut words, |w| w.kind == "cat" && w.tense == Tense::Past && w.body.is_none())
                .await;
            // one that speaks JSON is told a line each, as it is heard
            let json = json!({ "as": "echo", "argv": ["cat"], "speaks": "json" });
            town.talk(Word::new("join", Tense::Future, Body::of(json)));
            until(&mut words, |w| w.kind == "echo" && w.body.is_none()).await;
            for n in [1, 2] {
                town.talk(Word::new("echo", Tense::Future, Body::of(json!({ "n": n }))));
            }
            for n in [1, 2] {
                let back = until(&mut words, |w| w.kind == "echo" && w.tense == Tense::Past).await;
                assert_eq!(back.body, Parts::from(json!({ "n": n })));
            }
            std::fs::remove_file(&log).ok();
        });
    }

    // a connection says it is made, hears and is told in its language, and as the other end closes,
    // the past `connect` says so; one that cannot be made says why
    #[test]
    fn a_connection_talks_until_it_closes() {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        tokio::task::LocalSet::new().block_on(&rt, async {
            let dir = std::env::temp_dir().join(format!("town-connect-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            let town = Town::with_log(dir.join("past.log"));
            let line = Parts::from(json!({ "part": ["said", { "upto": "\n" }] }));
            town.languages.say("line", crate::language::Grammar::of(&line).unwrap());
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
            answer(&town);
            let mut words = town.words.subscribe();
            tokio::task::yield_now().await;
            let to = at.to_string_lossy();
            let connect = json!({ "as": "peer", "to": to, "speaks": "line", "about": "x" });
            town.talk(Word::new("connect", Tense::Future, Body::of(connect)));
            until(&mut words, |w| w.kind == "peer" && w.body.is_none()).await;
            let hello = until(&mut words, |w| w.kind == "peer").await;
            assert_eq!(hello.body, Parts::from(json!({ "said": "hello" })));
            town.talk(Word::new("peer", Tense::Future, Body::of(json!({ "said": "hi" }))));
            let back = until(&mut words, |w| w.kind == "peer" && w.tense == Tense::Past).await;
            assert_eq!(back.body, Parts::from(json!({ "said": "back hi" })));
            let ended = until(&mut words, |w| w.kind == "connect" && w.tense == Tense::Past).await;
            assert_eq!(
                (ended.body.value()["about"].as_str(), ended.body.value()["code"].as_i64()),
                (Some("x"), Some(0))
            );
            assert!(
                town.admits(&Word::new("peer", Tense::Past, Body::none())),
                "let go as it ends"
            );
            let gone = json!({ "as": "peer", "to": dir.join("none").to_string_lossy() });
            town.talk(Word::new("connect", Tense::Future, Body::of(gone)));
            let ended = until(&mut words, |w| w.kind == "connect" && w.tense == Tense::Past).await;
            assert!(ended.body.value()["out"].as_str().unwrap().starts_with("cannot connect to"));
            std::fs::remove_dir_all(&dir).ok();
        });
    }

    // a wait of any number of ms comes back, a float from a Lua division too; one without a
    // number a clock can count, below 0 or too big, is an error, not silence or a panic
    #[test]
    fn a_wait_comes_back_or_says_why_not() {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        tokio::task::LocalSet::new().block_on(&rt, async {
            let log = std::env::temp_dir().join(format!("town-after-{}.log", std::process::id()));
            let town = Town::with_log(log.clone());
            answer(&town);
            let mut words = town.words.subscribe();
            tokio::task::yield_now().await;
            town.talk(Word::new(
                "after",
                Tense::Future,
                Body::of(json!({ "ms": 250.0 / 10.0, "about": "x" })),
            ));
            town.talk(Word::new("after", Tense::Future, Body::of(json!({ "ms": -1 }))));
            town.talk(Word::new("after", Tense::Future, Body::of(json!({ "ms": 1e300 }))));
            // a last wait, of none: answered in order, so every refusal is said before it is back,
            // whichever of the two waits is back first
            town.talk(Word::new(
                "after",
                Tense::Future,
                Body::of(json!({ "ms": 0, "about": "end" })),
            ));
            let (mut back, mut ended, mut refused) = (false, false, 0);
            let heard = async {
                loop {
                    let w = match words.recv().await {
                        Ok(w) => w,
                        Err(RecvError::Lagged(_)) => continue,
                        Err(RecvError::Closed) => panic!("the town closed"),
                    };
                    let after = w.tense == Tense::Past && w.kind == "after";
                    back |= after && w.body.value()["about"] == "x";
                    ended |= after && w.body.value()["about"] == "end";
                    refused += (w.kind == "error" && w.body.value()["heard"] == "after") as usize;
                    if back && ended {
                        return;
                    }
                }
            };
            tokio::time::timeout(Duration::from_secs(30), heard).await.expect("never came");
            assert_eq!(refused, 2, "below 0, and too big to count");
            std::fs::remove_file(&log).ok();
        });
    }
}
