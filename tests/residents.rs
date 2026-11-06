//! A town as its residents, programs, speak to it: what none but town says, what one owns, and what
//! one spoken for keeps.
//!   cargo test --test residents

use town::body::Body;
use town::parts::Parts;
use town::town::{Listening, Town};
use town::word::{Tense, Word, ERROR};

use town::testing::{guard, resident, run, scratch, script, until};

/// every word said until one is `done`, that one aside
async fn until_done(words: &mut Listening) -> Vec<Word> {
    let mut said = Vec::new();
    loop {
        let w = until(words, |_| true).await;
        if w.kind == "done" {
            return said;
        }
        said.push(w);
    }
}

// town's own words are town's alone: a program that says an answer to `town/open`, or that another
// has come, is refused each, and none hears them. `done`, said after, shows all before it is in
#[test]
fn none_but_town_says_its_own() {
    let dir = scratch();
    script(
        &dir,
        "forger",
        r#"echo '{"kinds":["go"]}'
        while read -r w; do case "$w" in *'"go"'*)
            echo '{"kind":"town/open","tense":"past","body":{"paths":[]}}'
            echo '{"kind":"town/residents/someone","tense":"past"}'
            echo '{"kind":"done","tense":"past"}';;
        esac; done"#,
    );
    run(async {
        let town = Town::with_log(dir.log());
        let mut words = town.words.listen();
        town::residents::keep(&town, vec![dir.join("forger")]);
        until(&mut words, resident("forger", Tense::Present)).await;
        town.talk(Word::new("go", Tense::Past, Body::none()));
        let said = until_done(&mut words).await;
        let forged =
            |w: &&Word| w.tense == Tense::Past && w.kind.starts_with("town/") && w.kind != ERROR;
        assert!(!said.iter().any(|w| forged(&w)), "none heard: {said:?}");
        let refused = said.iter().filter(|w| w.kind == ERROR).count();
        assert_eq!(refused, 2, "each refused: {said:?}");
        town.reopen();
    });
}

// a kind has one join: one asked at a kind another's join has is refused, and the first stays
#[test]
fn a_kind_joined_is_one_askers() {
    let dir = scratch();
    let joins = |first: bool| {
        let join = r#"{"kind":"town/join","tense":"future","body":{"as":"x","argv":["cat"]}}"#;
        match first {
            true => format!(r#"echo '{{"kinds":["a"]}}'; echo '{join}'; exec cat >/dev/null"#),
            false => format!(
                r#"echo '{{"kinds":["go"]}}'
                while read -r w; do case "$w" in *'"go"'*) echo '{join}';; esac; done"#
            ),
        }
    };
    script(&dir, "a", &joins(true));
    script(&dir, "b", &joins(false));
    run(async {
        let town = Town::with_log(dir.log());
        let mut words = town.words.listen();
        town::residents::keep(&town, vec![dir.join("a"), dir.join("b")]);
        // both here, in either order: a's join, and b
        let (mut held, mut b) = (None, false);
        while held.is_none() || !b {
            let w = until(&mut words, |w| {
                resident("a/x", Tense::Present)(w) || resident("b", Tense::Present)(w)
            })
            .await;
            match resident("b", Tense::Present)(&w) {
                true => b = true,
                false => held = Some(w.kind),
            }
        }
        let held = held.unwrap();
        town.talk(Word::new("go", Tense::Past, Body::none()));
        let refused = until(&mut words, |w| {
            w.kind == ERROR || (w.kind == "town/join" && w.tense == Tense::Past)
        })
        .await;
        assert_eq!(refused.kind, ERROR, "b's join is refused: {refused:?}");
        assert!(town.facts(&held).contains_key(&held), "and a's stays");
        town.reopen();
    });
}

// one spoken for keeps what it said since its last `now` when its speaker says its fact again: its
// `now` after lets go only what it did not say
#[test]
fn one_spoken_for_keeps_what_it_said_as_its_fact_changes() {
    let dir = scratch();
    script(
        &dir,
        "speaker",
        r#"echo '{"kinds":["speaker"]}'
        echo '{"kind":"town/residents/a","tense":"present","body":{"own":["theme"]}}'
        echo '{"kind":"theme","tense":"present","body":"dark","by":"a"}'
        echo '{"kind":"town/residents/a","tense":"present","body":{"own":["theme"],"kinds":["x"]}}'
        echo '{"kind":"","tense":"past","by":"a"}'
        echo '{"kind":"done","tense":"past"}'
        exec cat >/dev/null"#,
    );
    run(async {
        let town = Town::with_log(dir.log());
        let mut words = town.words.listen();
        town::residents::keep(&town, vec![dir.join("speaker")]);
        until_done(&mut words).await;
        let theme = town.facts("theme").remove("theme");
        assert_eq!(theme.as_ref().map(Body::parts), Some(&Parts::from("dark")), "kept");
        town.reopen();
    });
}

// what one owned is held for it until it comes again: another in the opening may neither own it
// nor say it before then, a program as much as one at the square. The holder, come again, waits at
// a gate until both others have tried, so neither finds it here
#[test]
fn a_held_kind_is_its_holders_alone() {
    let dir = scratch();
    let gate = dir.join("gate");
    let gate_path = gate.display().to_string();
    script(
        &dir,
        "holder",
        &format!(
            r#"[ -p '{gate_path}' ] && read -r _ < '{gate_path}'
            echo '{{"kinds":["none"],"own":["x"]}}'
            echo '{{"kind":"x","tense":"present","body":1}}'
            exec cat >/dev/null"#
        ),
    );
    let first = dir.join("first");
    std::fs::create_dir_all(&first).unwrap();
    script(&first, "owns", r#"echo '{"kinds":["none"],"own":["x"]}'; exec cat >/dev/null"#);
    script(
        &first,
        "says",
        r#"echo '{"kinds":["none"]}'
        echo '{"kind":"x","tense":"present","body":2}'
        exec cat >/dev/null"#,
    );
    run(async {
        let town = Town::with_log(dir.log());
        let mut words = town.words.listen();
        town::residents::keep(&town, vec![dir.join("holder")]);
        until(&mut words, |w| w.kind == "x").await;
        town.reopen();
        let fifo = std::ffi::CString::new(gate_path.as_str()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0, "a gate");
        let paths = vec![first.join("owns"), first.join("says"), dir.join("holder")];
        town::residents::keep(&town, paths);
        let why = |w: &Word| w.body.part("message").and_then(Parts::text).unwrap_or("").to_string();
        let mut held = Vec::new();
        while held.len() < 2 {
            let e = until(&mut words, |w| w.kind == ERROR).await;
            held.push(why(&e));
        }
        assert!(held.iter().all(|m| m.contains("held")), "none owns it, nor says it: {held:?}");
        // the holder, let in, comes again: what it says of `x` is as it is, so no word, and `x` is
        // still its own
        // it opens as the holder reads: under the guard, so one that never comes fails
        let opened = tokio::task::spawn_blocking(move || std::fs::write(&gate, "\n"));
        guard(opened).await.unwrap().unwrap();
        until(&mut words, resident("holder", Tense::Present)).await;
        assert_eq!(town.facts("x").remove("x"), Some(Body::of(1i64)), "it stays the holder's");
        town.reopen();
    });
}
