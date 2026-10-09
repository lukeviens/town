//! work — town causing the world to change. A future `work` runs outside without blocking the
//! runtime; the past `work` carries what it cost and whatever the caller sent with it.

use std::rc::Rc;

use serde_json::{json, Value};

use crate::town::Town;
use crate::word::{Tense, Word};

/// Obey every future `work`. Engine, not a resident: it outlives a reopen.
pub fn obey(town: &Rc<Town>) {
    let town = town.clone();
    tokio::task::spawn_local(async move {
        let mut words = town.words.subscribe();
        loop {
            match words.recv().await {
                Ok(w) if w.kind == "work" && w.tense == Tense::Future => {
                    if let Some(cmd) = w.body.get("cmd").and_then(Value::as_str) {
                        let (cmd, body, town) = (cmd.to_string(), w.body.clone(), town.clone());
                        tokio::task::spawn_local(async move { done(town, cmd, body).await });
                    }
                }
                Ok(_) => {}
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => break,
            }
        }
    });
}

async fn done(town: Rc<Town>, cmd: String, body: Value) {
    // `at` names a program town has joined: write the line to it rather than spawn a shell. The
    // answer, if there is one, comes back on that join as its own word — this says only that the
    // line went out.
    if let Some(at) = body.get("at").and_then(Value::as_str).map(String::from) {
        let sent = town.joins.borrow().get(&at).map(|tx| tx.send(cmd).is_ok()).unwrap_or(false);
        // the caller's fields ride back, as they do for a shell. A line is written in the order it
        // was sent, and that order is the only thing that can match a reply to what asked for it.
        let mut body = body;
        if let Some(m) = body.as_object_mut() {
            m.remove("cmd");
            m.insert("ms".into(), json!(0));
            m.insert("code".into(), json!(if sent { 0 } else { -1 }));
        }
        town.talk(Word { kind: "work".into(), tense: Tense::Past, body, at: 0 });
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
    town.talk(Word { kind: "work".into(), tense: Tense::Past, body, at: 0 });
}
