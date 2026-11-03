//! The examples, opened as a town and spoken to: each answers as its header says, so none can go
//! stale.
//!   cargo test --test examples

use std::path::Path;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use town::residents;
use town::town::Town;
use town::word::{Tense, Word};

#[path = "../src/testing.rs"]
#[allow(dead_code)] // the examples need only some of it
mod testing;
use testing::until;

fn word(kind: &str, tense: Tense, body: Value) -> Word {
    Word::new(kind, tense, body)
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
        town::work::answer(&town);
        let mut words = town.words.subscribe();
        let mut shouts = town.words.subscribe(); // shout begins as the town opens
        residents::open(&lua, &town, &[Path::new(env!("CARGO_MANIFEST_DIR")).join("examples")]);
        until(&mut words, |w| w.kind == "notes").await; // notes.lua is up: its file, said

        // hello: a hi, the time it asked for, from clock, and a bye once its wait has gone
        let asked = Instant::now();
        town.talk(word("hello", Tense::Present, json!({ "from": "me" })));
        let hi = until(&mut words, |w| w.kind == "hi").await;
        assert_eq!((hi.tense, &hi.body), (Tense::Past, &json!({ "to": "me" })));
        let clock = until(&mut words, |w| w.kind == "clock").await;
        assert_eq!(clock.tense, Tense::Present);
        assert_eq!(clock.body["now"].as_str().unwrap().len(), "12:34:56".len());
        let bye = until(&mut words, |w| w.kind == "bye").await;
        assert_eq!(bye.body, json!({ "to": "me" }));
        assert!(asked.elapsed() >= Duration::from_millis(500), "bye waits its 500 ms");

        // ask: clock answers the time with the asker's id
        town.talk(word("time", Tense::Future, json!({ "id": "q1" })));
        let answer = until(&mut words, |w| w.kind == "time" && w.tense == Tense::Past).await;
        assert_eq!(answer.body["id"], "q1");

        // notes: the line is written, the file said again, once, and `noted` is notes.lua's alone
        town.talk(word("note", Tense::Past, json!({ "text": "buy milk" })));
        until(&mut words, |w| w.kind == "noted" && w.body["text"] == "buy milk").await;
        until(&mut words, |w| w.kind == "notes" && w.body == "buy milk\n").await;
        // and once: the file said next is the next note's
        town.talk(word("note", Tense::Past, json!({ "text": "eggs" })));
        let next = until(&mut words, |w| w.kind == "notes").await;
        assert_eq!(next.body, "buy milk\neggs\n");
        assert!(!town.admits(&word("noted", Tense::Past, Value::Null)));

        // echo: what goes into `cat` comes back out
        town.talk(word("say", Tense::Future, json!({ "text": "hi" })));
        let echoed = until(&mut words, |w| w.kind == "echoed").await;
        assert_eq!(echoed.body, json!({ "text": "hi" }));

        // shout, a program: asked clock the time as it began, and shouts what it is told
        let shouting = until(&mut shouts, |w| w.kind == "shouting").await.body;
        assert_eq!(shouting["since"].as_str().unwrap().len(), "12:34:56".len());
        town.talk(word("shout", Tense::Future, json!({ "text": "hi" })));
        let shouted = until(&mut words, |w| w.kind == "shouted").await;
        assert_eq!(shouted.body, json!({ "text": "HI" }));

        for h in town.tasks.borrow_mut().drain(..) {
            h.abort();
        }
    });
    std::fs::remove_dir_all(&dir).ok();
}
