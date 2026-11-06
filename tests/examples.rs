//! The examples, opened as a town and spoken to: each answers as its header says, so none can go
//! stale.
//!   cargo test --test examples

use std::path::Path;

use serde_json::{json, Value};
use town::body::Body;
use town::parts::Parts;
use town::residents;
use town::town::Town;
use town::word::{Tense, Word};

use town::testing::{run, scratch, until, world_finding};

/// a body, to look into
trait Look {
    fn value(&self) -> Value;
}

impl Look for Body {
    fn value(&self) -> Value {
        serde_json::to_value(self.parts()).unwrap()
    }
}

/// a word, its body as json
fn word(kind: &str, tense: Tense, body: Value) -> Word {
    Word::new(kind, tense, Body::of(body))
}

// each example, opened as one town, answers as its header says
#[test]
fn the_examples_answer_as_they_say() {
    let dir = scratch();
    // its world: `mirror` on its PATH, and where notes.lua keeps its file
    let mut world = world_finding(Path::new(env!("CARGO_BIN_EXE_mirror")));
    world.insert("TMPDIR".into(), dir.as_os_str().into());
    run(async {
        let town = Town::in_world(dir.log(), world);
        let mut words = town.words.listen();
        // what shout says, heard from the start
        let mut shouts = town.words.listen();
        // town's own residents, the languages they speak among them, then them
        let examples = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples");
        residents::open(&town, &[residents::own(), vec![examples]].concat());
        // notes.lua is up: its file, said
        until(&mut words, |w| w.kind == "notes").await;
        // each Lua example is here before it is talked to: `lua` passes a file only what is said
        // after it came
        let here = |name: &str| {
            let at = format!("/examples/{name}");
            town.facts("town/residents/").keys().any(|kind| kind.ends_with(&at))
        };
        for name in ["clock.lua", "echo.lua", "hi.lua", "notes.lua"] {
            while !here(name) {
                until(&mut words, |_| true).await;
            }
        }

        // hello: a hi, the time it asked for, from clock, and a bye once its wait, a `sleep`, ends.
        // clock and bye each wait on a program, so they come in either order
        town.talk(word("hello", Tense::Past, json!({ "from": "me" })));
        let hi = until(&mut words, |w| w.kind == "hi").await;
        assert_eq!(hi.tense, Tense::Past);
        assert_eq!(hi.body, Parts::from(json!({ "to": "me" })));
        let (mut clock, mut bye) = (None, None);
        while clock.is_none() || bye.is_none() {
            let w = until(&mut words, |w| w.kind == "clock" || w.kind == "bye").await;
            match w.kind.as_str() {
                "clock" => clock = Some(w),
                _ => bye = Some(w),
            }
        }
        let (clock, bye) = (clock.unwrap(), bye.unwrap());
        assert_eq!(clock.tense, Tense::Present);
        assert_eq!(clock.body.value()["now"].as_str().unwrap().len(), "12:34:56".len());
        assert_eq!(bye.body, Parts::from(json!({ "to": "me" })));

        // ask: clock answers the time with the asker's id
        town.talk(word("time", Tense::Future, json!({ "id": "q1" })));
        let answer = until(&mut words, |w| w.kind == "time" && w.tense == Tense::Past).await;
        assert_eq!(answer.body.value()["id"], "q1");

        // notes: the line is written, the file said again, once, and `noted` is notes.lua's alone
        town.talk(word("note", Tense::Past, json!({ "text": "buy milk" })));
        until(&mut words, |w| w.kind == "noted" && w.body.value()["text"] == "buy milk").await;
        let size = |w: &Word| w.body.value()["size"].as_u64();
        until(&mut words, |w| w.kind == "notes" && size(w) == Some("buy milk\n".len() as u64))
            .await;
        // and once: the file said next is the next note's
        town.talk(word("note", Tense::Past, json!({ "text": "eggs" })));
        let next = until(&mut words, |w| w.kind == "notes").await;
        assert_eq!(size(&next), Some("buy milk\neggs\n".len() as u64));
        assert!(town
            .may(&word("noted", Tense::Past, Value::Null), &town.who("square", true))
            .is_err());

        // echo: what goes into `cat` comes back out
        town.talk(word("say", Tense::Future, json!({ "text": "hi" })));
        let echoed = until(&mut words, |w| w.kind == "echoed").await;
        assert_eq!(echoed.body, Parts::from(json!({ "text": "hi" })));

        // shout, a program: shouts what it is told, and asks clock the time as it first does
        town.talk(word("shout", Tense::Future, json!({ "text": "hi" })));
        let shouted = until(&mut words, |w| w.kind == "shouted").await;
        assert_eq!(shouted.body, Parts::from(json!({ "text": "HI" })));
        let shouting = until(&mut shouts, |w| w.kind == "shouting").await.body.value();
        assert_eq!(shouting["since"].as_str().unwrap().len(), "12:34:56".len());

        town.reopen();
    });
}
