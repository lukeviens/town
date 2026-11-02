//! The examples, opened as a town and spoken to: each answers as its header says, so none can go
//! stale.
//!   cargo test --test examples

use std::path::Path;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tokio::sync::broadcast::Receiver;
use town::residents;
use town::town::Town;
use town::word::{Tense, Word};

fn word(kind: &str, tense: Tense, body: Value) -> Word {
    Word::new(kind, tense, body)
}

/// the first word that `is` within a few seconds, letting the town run while it waits
async fn heard(words: &mut Receiver<Word>, is: impl Fn(&Word) -> bool) -> Word {
    let until = Instant::now() + Duration::from_secs(5);
    while Instant::now() < until {
        while let Ok(w) = words.try_recv() {
            if is(&w) {
                return w;
            }
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("not heard in time");
}

#[test]
fn the_examples_answer_as_they_say() {
    let dir = std::env::temp_dir().join(format!("town-examples-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::env::set_var("TMPDIR", &dir); // where notes.lua keeps its file
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    tokio::task::LocalSet::new().block_on(&rt, async {
        let town = Town::with_log(dir.join("past.log"));
        let lua = mlua::Lua::new();
        residents::install(&lua, &town).unwrap();
        town::work::obey(&town);
        let mut words = town.words.subscribe();
        residents::open(&lua, &town, &[Path::new(env!("CARGO_MANIFEST_DIR")).join("examples")]);
        heard(&mut words, |w| w.kind == "notes").await; // notes.lua is up: its file, said

        // hello: a hi, the time it asked for, from clock, and a bye once its wait has gone
        let asked = Instant::now();
        town.talk(word("hello", Tense::Present, json!({ "from": "me" })));
        let hi = heard(&mut words, |w| w.kind == "hi").await;
        assert_eq!((hi.tense, &hi.body), (Tense::Past, &json!({ "to": "me" })));
        let clock = heard(&mut words, |w| w.kind == "clock").await;
        assert_eq!(clock.tense, Tense::Present);
        assert_eq!(clock.body["now"].as_str().unwrap().len(), "12:34:56".len());
        let bye = heard(&mut words, |w| w.kind == "bye").await;
        assert_eq!(bye.body, json!({ "to": "me" }));
        assert!(asked.elapsed() >= Duration::from_millis(500), "bye waits its 500 ms");

        // ask: clock answers the time with the asker's id
        town.talk(word("time", Tense::Future, json!({ "id": "q1" })));
        let answer = heard(&mut words, |w| w.kind == "time" && w.tense == Tense::Past).await;
        assert_eq!(answer.body["id"], "q1");

        // notes: the line is written, the file said again, once, and `noted` is notes.lua's alone
        town.talk(word("note", Tense::Past, json!({ "text": "buy milk" })));
        heard(&mut words, |w| w.kind == "noted" && w.body["text"] == "buy milk").await;
        heard(&mut words, |w| w.kind == "notes" && w.body == "buy milk\n").await;
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(std::iter::from_fn(|| words.try_recv().ok()).all(|w| w.kind != "notes"));
        assert!(!town.admits(&word("noted", Tense::Past, Value::Null)));

        // echo: what goes into `cat` comes back out
        town.talk(word("say", Tense::Future, json!({ "text": "hi" })));
        let echoed = heard(&mut words, |w| w.kind == "echoed").await;
        assert_eq!(echoed.body, json!({ "text": "hi" }));

        for h in town.watchers.borrow_mut().drain(..) {
            h.abort();
        }
    });
    std::fs::remove_dir_all(&dir).ok();
}
