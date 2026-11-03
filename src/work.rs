//! work — town causing the world to change. A future `work` runs outside without blocking the
//! runtime; the past `work` carries what it cost and whatever the caller sent with it. A future
//! `after` is town keeping time: once its `ms` have gone, the past `after` carries what was sent. A
//! future `join` is a program town keeps talking to until it ends: then the past `join` carries how
//! it ended, and what was sent.

use std::rc::Rc;
use std::time::Duration;

use serde_json::{json, Value};
use tokio::io::AsyncWriteExt;
use tokio::process::{ChildStdin, ChildStdout};
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::mpsc;

use crate::town::{Lines, Town};
use crate::word::{Tense, Word};

/// Answer every future `work`, `after` and `join` with a past word of its kind. Engine, not a
/// resident: it outlives a reopen.
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
                let ms = w.body.get("ms").and_then(Value::as_f64);
                match ms.and_then(|ms| Duration::try_from_secs_f64(ms / 1000.0).ok()) {
                    Some(wait) => {
                        tokio::task::spawn_local(waited(town.clone(), wait, w.body.clone()));
                    }
                    None => town.error("after", Some("after"), "a wait needs ms: 0 or more".into()),
                }
                continue;
            }
            if w.kind == "join" {
                join(&town, w.body);
                continue;
            }
            // a future word of a joined kind is a line wanted for its program
            if let Some((tx, _)) = town.joins.borrow().get(&w.kind) {
                if let Some(line) = line(&w.body) {
                    tx.send(line).ok();
                }
                continue;
            }
            if w.kind != "work" {
                continue;
            }
            let Some(cmd) = w.body.get("cmd").and_then(Value::as_str) else { continue };
            tokio::task::spawn_local(done(town.clone(), cmd.to_string(), w.body.clone()));
        }
    });
}

/// Join a program as a kind: each line it says is a past word of that kind, and each future word
/// of that kind is a line written to it. Joining a kind again ends the one before. A joined
/// program's lines are what its kind says, so only town says that kind while it runs. A reopen ends
/// it: a resident joins again as it opens.
fn join(town: &Rc<Town>, body: Value) {
    let kind = body.get("as").and_then(Value::as_str).map(String::from);
    let argv: Vec<String> = body
        .get("argv")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).map(String::from).collect())
        .unwrap_or_default();
    let Some(kind) = kind.filter(|_| !argv.is_empty()) else {
        return town.error("join", Some("join"), "a join needs `as`, and `argv` to run".into());
    };
    // town's own words mean what town makes them mean: a program cannot speak as one
    if Town::machinery().contains_key(&kind) {
        return town.error("join", Some("join"), format!("`{kind}` is town's own, not to join"));
    }
    let (tx, rx) = mpsc::unbounded_channel();
    let run =
        tokio::task::spawn_local(joined(town.clone(), kind.clone(), argv, tx.clone(), rx, body));
    match town.joins.borrow_mut().insert(kind.clone(), (tx, run.abort_handle())) {
        Some((_, before)) => before.abort(),
        None => town.hold(&kind),
    }
    let mut tasks = town.tasks.borrow_mut();
    tasks.retain(|t| !t.is_finished()); // a resident may join again and again: keep only what runs
    tasks.push(run);
}

/// A joined program, from its start to its end. It is said as a bodiless word as it starts, since
/// whatever was said to one before it, a subscription, a mode, is gone with that one. As it ends,
/// its kind is let go, if it was not joined again, and the past `join` says how it ended.
async fn joined(
    town: Rc<Town>,
    kind: String,
    argv: Vec<String>,
    me: mpsc::UnboundedSender<String>,
    mut lines_in: mpsc::UnboundedReceiver<String>,
    body: Value,
) {
    // a reopen, or a join again, ends the run, and the program goes with it, and all it started:
    // a group of its own, as a resident's is
    let child = tokio::process::Command::new(&argv[0])
        .args(&argv[1..])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .process_group(0)
        .kill_on_drop(true)
        .spawn();
    let (code, out) = match child {
        Err(e) => (-1, Some(format!("cannot join {}: {e}", argv[0]))),
        Ok(mut child) => {
            let _group = crate::residents::Group::of(&town, &child);
            if let (Some(out), Some(inp)) = (child.stdout.take(), child.stdin.take()) {
                town.talk(Word::new(&kind, Tense::Past, Value::Null));
                converse(&town, &kind, out, inp, &mut lines_in).await;
            }
            // what it says has ended; so, or soon, has it
            let status = child.wait().await;
            (status.ok().and_then(|s| s.code()).unwrap_or(-1), None)
        }
    };
    let mine = town.joins.borrow().get(&kind).is_some_and(|(tx, _)| tx.same_channel(&me));
    if mine {
        town.joins.borrow_mut().remove(&kind);
        town.free(&kind);
    }
    let mut body = body;
    if let Some(m) = body.as_object_mut() {
        m.insert("code".into(), json!(code));
        if let Some(out) = out {
            m.insert("out".into(), json!(out));
        }
    }
    town.talk(Word::new("join", Tense::Past, body));
}

/// Lines both ways with a joined program, until it hangs up: each it says is a past word of
/// `kind`, and each `work` sends it is written to it.
async fn converse(
    town: &Town,
    kind: &str,
    out: ChildStdout,
    mut inp: ChildStdin,
    lines_in: &mut mpsc::UnboundedReceiver<String>,
) {
    let mut lines = Lines::new(out);
    loop {
        tokio::select! {
            line = lines.next() => match line {
                Some(line) => town.talk(Word::new(kind, Tense::Past, Value::String(line))),
                None => return,
            },
            Some(line) = lines_in.recv() => {
                if inp.write_all(format!("{line}\n").as_bytes()).await.is_err() {
                    return;
                }
            }
        }
    }
}

/// What a future word of a joined kind writes to its program: text as it is, anything else as
/// JSON. A bodiless one writes nothing.
fn line(body: &Value) -> Option<String> {
    match body {
        Value::Null => None,
        Value::String(s) => Some(s.clone()),
        v => Some(v.to_string()),
    }
}

/// The time asked for, gone: said back as a past `after`, with everything the caller sent.
async fn waited(town: Rc<Town>, wait: Duration, body: Value) {
    tokio::time::sleep(wait).await;
    town.talk(Word::new("after", Tense::Past, body));
}

/// A command run in a shell: the past `work` carries what it said, how it ended, and what it
/// cost.
async fn done(town: Rc<Town>, cmd: String, body: Value) {
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
    // the caller's own fields ride back with the answer, so it can match on them (`on`'s `by`)
    let mut body = body;
    if let Some(m) = body.as_object_mut() {
        m.remove("cmd");
        m.insert("code".into(), json!(code));
        m.insert("out".into(), json!(stdout));
        m.insert("ms".into(), json!(ms));
    }
    town.talk(Word::new("work", Tense::Past, body));
}

#[cfg(test)]
mod tests {
    use super::*;

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
                    json!({ "as": "cat", "argv": argv, "about": about }),
                )
            };
            town.talk(join(json!(["cat"]), "first"));
            until(&mut words, |w| w.kind == "cat" && w.body.is_null()).await;
            assert!(
                !town.admits(&Word::new("cat", Tense::Past, Value::Null)),
                "town's, while joined"
            );
            town.talk(Word::new("cat", Tense::Future, json!("hi")));
            until(&mut words, |w| w.kind == "cat" && w.body == "hi").await;
            // joined again: the first ends, and only the second answers
            town.talk(join(json!(["sh", "-c", "read -r l; echo got $l; exit 3"]), "second"));
            until(&mut words, |w| w.kind == "cat" && w.body.is_null()).await;
            town.talk(Word::new("cat", Tense::Future, json!("x")));
            until(&mut words, |w| w.kind == "cat" && w.body == "got x").await;
            let ended = until(&mut words, |w| w.kind == "join" && w.tense == Tense::Past).await;
            assert_eq!(
                (ended.body["about"].as_str(), ended.body["code"].as_i64()),
                (Some("second"), Some(3))
            );
            assert!(town.admits(&Word::new("cat", Tense::Past, Value::Null)), "let go as it ends");
            town.talk(join(json!(["/no/such/program"]), "none"));
            let none = until(&mut words, |w| w.kind == "join" && w.tense == Tense::Past).await;
            assert_eq!(none.body["code"], -1);
            assert!(none.body["out"].as_str().unwrap().contains("/no/such/program"));
            town.talk(join(json!([]), "empty"));
            let bad = until(&mut words, |w| w.kind == "error").await;
            assert_eq!(bad.body["heard"], "join");
            // nor as one of town's own words
            let work = json!({ "as": "work", "argv": ["cat"] });
            town.talk(Word::new("join", Tense::Future, work));
            let bad = until(&mut words, |w| w.kind == "error").await;
            assert!(bad.body["message"].as_str().unwrap().contains("town's own"));
            town.tasks.borrow_mut().drain(..).for_each(|h| h.abort());
            std::fs::remove_file(&log).ok();
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
                json!({ "ms": 250.0 / 10.0, "about": "x" }),
            ));
            town.talk(Word::new("after", Tense::Future, json!({ "ms": -1 })));
            town.talk(Word::new("after", Tense::Future, json!({ "ms": 1e300 })));
            // a last wait, of none: answered in order, so every refusal is said before it is back,
            // whichever of the two waits is back first
            town.talk(Word::new("after", Tense::Future, json!({ "ms": 0, "about": "end" })));
            let (mut back, mut ended, mut refused) = (false, false, 0);
            let heard = async {
                loop {
                    let w = match words.recv().await {
                        Ok(w) => w,
                        Err(RecvError::Lagged(_)) => continue,
                        Err(RecvError::Closed) => panic!("the town closed"),
                    };
                    let after = w.tense == Tense::Past && w.kind == "after";
                    back |= after && w.body["about"] == "x";
                    ended |= after && w.body["about"] == "end";
                    refused += (w.kind == "error" && w.body["heard"] == "after") as usize;
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
