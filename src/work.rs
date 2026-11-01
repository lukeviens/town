//! work — town causing the world to change. A future `work` runs outside without blocking the
//! runtime; the past `work` carries what it cost and whatever the caller sent with it. A future
//! `after` is town keeping time: once its `ms` have gone, the past `after` carries what was sent.

use std::rc::Rc;
use std::time::Duration;

use serde_json::{json, Value};
use tokio::sync::broadcast::error::RecvError;

use crate::town::Town;
use crate::word::{Tense, Word};

/// Obey every future `work` and `after`. Engine, not a resident: it outlives a reopen.
pub fn obey(town: &Rc<Town>) {
    let town = town.clone();
    tokio::task::spawn_local(async move {
        let mut words = town.words.subscribe();
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
            if w.kind != "work" {
                continue;
            }
            let Some(cmd) = w.body.get("cmd").and_then(Value::as_str) else { continue };
            tokio::task::spawn_local(done(town.clone(), cmd.to_string(), w.body.clone()));
        }
    });
}

/// The time asked for, gone: said back as a past `after`, with everything the caller sent.
async fn waited(town: Rc<Town>, wait: Duration, body: Value) {
    tokio::time::sleep(wait).await;
    town.talk(Word::new("after", Tense::Past, body));
}

async fn done(town: Rc<Town>, cmd: String, body: Value) {
    // `join` names a program town has joined: write the line to it rather than spawn a shell. The
    // answer, if there is one, comes back on that join as its own word — this says only that the
    // line went out.
    if let Some(join) = body.get("join").and_then(Value::as_str).map(String::from) {
        let sent = town.joins.borrow().get(&join).map(|tx| tx.send(cmd).is_ok()).unwrap_or(false);
        // the caller's fields ride back, as they do for a shell. A line is written in the order it
        // was sent, and that order is the only thing that can match a reply to what asked for it.
        let mut body = body;
        if let Some(m) = body.as_object_mut() {
            m.remove("cmd");
            m.insert("ms".into(), json!(0));
            m.insert("code".into(), json!(if sent { 0 } else { -1 }));
            if !sent {
                m.insert("out".into(), json!(format!("no program joined as {join}")));
            }
        }
        town.talk(Word::new("work", Tense::Past, body));
        return;
    }
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

    // a wait of any number of ms comes back, a float from a Lua division too; one without a
    // number a clock can count, below 0 or too big, is an error, not silence or a panic
    #[test]
    fn a_wait_comes_back_or_says_why_not() {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        tokio::task::LocalSet::new().block_on(&rt, async {
            let log = std::env::temp_dir().join(format!("town-after-{}.log", std::process::id()));
            let town = Town::with_log(log.clone());
            obey(&town);
            let mut words = town.words.subscribe();
            tokio::task::yield_now().await;
            town.talk(Word::new(
                "after",
                Tense::Future,
                json!({ "ms": 250.0 / 10.0, "about": "x" }),
            ));
            town.talk(Word::new("after", Tense::Future, json!({ "ms": -1 })));
            town.talk(Word::new("after", Tense::Future, json!({ "ms": 1e300 })));
            tokio::time::sleep(Duration::from_millis(100)).await;
            let said: Vec<Word> = std::iter::from_fn(|| words.try_recv().ok())
                .filter(|w| w.tense == Tense::Past)
                .collect();
            assert!(said.iter().any(|w| w.kind == "after" && w.body["about"] == "x"));
            let refused = said.iter().filter(|w| w.kind == "error" && w.body["heard"] == "after");
            assert_eq!(refused.count(), 2, "below 0, and too big to count");
            std::fs::remove_file(&log).ok();
        });
    }
}
