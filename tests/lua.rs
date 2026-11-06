//! `lua`, town's own resident that speaks for the Lua files, as a town runs it: a program, named
//! among the paths a test town opens with, beside the Lua files it speaks for.

use std::path::{Path, PathBuf};
use std::rc::Rc;

use town::body::Body;
use town::parts::Parts;
use town::testing::{probe, resident, run, scratch, script, settled, until, Scratch};
use town::town::RESIDENTS;
use town::town::{Listening, Town};
use town::word::{Tense, Word, ERROR};

/// `lua`, built
fn lua() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_lua"))
}

/// a path as a place: whole, but its first `/`
fn trimmed(path: &Path) -> String {
    town::residents::whole(path).to_string_lossy().trim_start_matches('/').to_string()
}

/// the place of the resident a Lua file is: under `lua`'s, by its path
fn place(path: &Path) -> String {
    format!("{}/{}", trimmed(&lua()), trimmed(path))
}

/// where the fact of the resident a Lua file is would be
fn fact(path: &Path) -> String {
    format!("{RESIDENTS}/{}", place(path))
}

/// a town of a test's own, `lua` speaking for the Lua files of `paths`, and its probe, heard from
/// before it opens
fn town(dir: &Scratch, paths: &[PathBuf]) -> (Rc<Town>, Listening) {
    let town = Town::with_log(dir.log());
    let words = town.words.listen();
    town::residents::keep(&town, probed(dir, paths));
    (town, words)
}

/// `lua`, then `paths`, then the probe last, so every library comes before it; a probe within one
/// of `paths` is read with it
fn probed(dir: &Path, paths: &[PathBuf]) -> Vec<PathBuf> {
    let probe = probe(dir);
    let mut all = vec![lua()];
    all.extend(paths.iter().cloned());
    if !paths.iter().any(|p| probe.starts_with(p)) {
        all.push(probe);
    }
    all
}

/// until the Lua file at `path` has come, as a resident
async fn came(words: &mut Listening, path: &Path) {
    until(words, resident(&place(path), Tense::Present)).await;
}

/// a word of `kind` in `tense`, with no body
fn said(kind: &str, tense: Tense) -> Word {
    Word::new(kind, tense, Body::none())
}

/// a save of `path`, as `reload` asks it: written, as it is, if it is there, so it is another
/// version of it
fn saved(town: &Town, path: &Path) {
    if let Ok(was) = std::fs::read(path) {
        std::fs::write(path, was).unwrap();
    }
    let path = Parts::map([("path", path.to_string_lossy().as_ref())]);
    town.talk(Word::new("town/open", Tense::Future, Body::of(path)));
}

/// the number of the resident a Lua file is, if it is here
fn id(town: &Town, path: &Path) -> Option<u64> {
    let kind = fact(path);
    town.facts(&kind).remove(&kind).and_then(|b| part(&b, "id").parse().ok())
}

/// a part of a body, as text: a number as it is written, nothing as ""
fn part(body: &Body, name: &str) -> String {
    match body.part(name) {
        Some(Parts::Text(t) | Parts::Number(t)) => t.clone(),
        _ => String::new(),
    }
}

/// the message of an error word
fn message(w: &Word) -> String {
    part(&w.body, "message")
}
// a file that returns nothing is a library, and the residents after it are written with what it
// defined
#[test]
fn a_library_comes_before_the_residents_that_use_it() {
    run(async {
        let dir = scratch();
        std::fs::create_dir_all(dir.join("residents")).unwrap();
        std::fs::write(dir.join("lib.lua"), r#"function greeting() return "hello" end"#).unwrap();
        let echo = dir.join("residents/echo.lua");
        let pongs =
            r#"return react { on("ping", function() return event("pong", greeting()) end) }"#;
        std::fs::write(&echo, pongs).unwrap();
        let (town, mut words) = town(&dir, &[dir.join("lib.lua"), dir.join("residents")]);
        came(&mut words, &echo).await;
        town.talk(said("ping", Tense::Past));
        assert_eq!(until(&mut words, |w| w.kind == "pong").await.body, "hello");
        town.reopen();
    });
}

// work asked badly, with no list of strings, fails where it was asked: the asking resident's own
// error
#[test]
fn a_bad_work_is_its_residents_error() {
    run(async {
        let dir = scratch();
        let bad = ["'date'", "42", "{}", "{ 1 }"];
        let files: Vec<PathBuf> = bad
            .iter()
            .enumerate()
            .map(|(i, ms)| {
                let path = dir.join(format!("waiter{i}.lua"));
                let waits = format!(r#"function() return work({ms}, {{ about = "x" }}) end"#);
                let asks = format!(r#"return react {{ on("ping", {waits}) }}"#);
                std::fs::write(&path, asks).unwrap();
                path
            })
            .collect();
        let (town, mut words) = town(&dir, &files);
        for f in &files {
            came(&mut words, f).await;
        }
        town.talk(said("ping", Tense::Past));
        let mut from = Vec::new();
        for _ in &bad {
            let e = until(&mut words, |w| w.kind == ERROR).await;
            from.push(part(&e.body, "from"));
        }
        from.sort();
        assert_eq!(from, ["waiter0.lua", "waiter1.lua", "waiter2.lua", "waiter3.lua"]);
        town.reopen();
    });
}

// a resident that fails, by a Lua error or a non-word talked or answered, is said as a
// `town/error`: who, hearing what, and why. One that fails hearing a `town/error` is said too, but
// marked `answering`, and told to none that hears errors: two that hear them make no more.
#[test]
fn a_failure_is_an_error_word() {
    run(async {
        let dir = scratch();
        let fails = [
            ("lua-error", r#"error("no " .. w.kind)"#, "no ping"),
            ("talks", r#"talk({ kind = "x", tense = "psat" })"#, "talked a non-word"),
            ("answers", r#"return { kind = "x", tense = "psat" }"#, "answered with a non-word"),
        ];
        let files: Vec<PathBuf> = fails
            .iter()
            .map(|(name, fails, _)| {
                let path = dir.join(format!("{name}.lua"));
                let file = format!(
                    r#"return react {{
                        on("ping", function(w) {fails} end),
                        on("town/error", function(w) {fails} end),
                    }}"#
                );
                std::fs::write(&path, file).unwrap();
                path
            })
            .collect();
        let (town, mut words) = town(&dir, &files);
        for f in &files {
            came(&mut words, f).await;
        }
        town.talk(said("ping", Tense::Past));
        let all: Vec<Word> =
            settled(&town, &mut words).await.into_iter().filter(|w| w.kind == ERROR).collect();
        let answering = |e: &&Word| e.body.part("answering").is_some();
        let (marked, errors): (Vec<&Word>, Vec<&Word>) = all.iter().partition(answering);
        assert_eq!(errors.len(), fails.len(), "one each");
        // each fails at the others' errors, once, and those are told to none
        assert_eq!(marked.len(), fails.len() * (fails.len() - 1), "one at each other's");
        assert!(marked.iter().all(|e| part(&e.body, "heard") == ERROR));
        for (name, _, why) in fails {
            let e = errors.iter().find(|e| part(&e.body, "from") == format!("{name}.lua")).unwrap();
            assert_eq!(part(&e.body, "heard"), "ping", "{why}");
            assert!(message(e).contains(why), "{}", message(e));
        }
        town.reopen();
    });
}

// a file that fails to open has done nothing: what it talked as it was read is not said, its
// joins among them; one that opens says what it talked, in order, once it has come
#[test]
fn a_file_that_fails_to_open_does_nothing() {
    run(async {
        let dir = scratch();
        let talks = r#"talk(event("first")); talk(join("cat", { "cat" })); talk(event("second"))"#;
        let (a, b) = (dir.join("a-fails.lua"), dir.join("b-opens.lua"));
        std::fs::write(&a, format!("{talks}; return {{ nope = 1 }}")).unwrap();
        std::fs::write(&b, format!(r#"{talks}; return {{ own = {{ "x" }} }}"#)).unwrap();
        let (town, mut words) = town(&dir, &[a, b.clone()]);
        came(&mut words, &b).await;
        let said = settled(&town, &mut words).await;
        let at = |kind: &str| said.iter().position(|w| w.kind == kind).unwrap();
        assert_eq!(said.iter().filter(|w| w.kind == "first").count(), 1, "only b's: {said:?}");
        assert!(at("first") < at("town/join") && at("town/join") < at("second"), "{said:?}");
        town.reopen();
    });
}

// two that hear `town/error` and answer each with what is refused make no error of it, as they
// would each other's without end
#[test]
fn two_that_answer_errors_make_none() {
    run(async {
        let dir = scratch();
        let answers = r#"return react { on("town/error", function() return event("mine") end) }"#;
        std::fs::write(dir.join("a.lua"), answers).unwrap();
        std::fs::write(dir.join("b.lua"), answers).unwrap();
        std::fs::write(dir.join("c.lua"), r#"return { own = { "mine" } }"#).unwrap();
        let (town, mut words) = town(&dir, &[dir.to_path_buf()]);
        came(&mut words, &dir.join("c.lua")).await;
        town.error(&town.who("first", false), None, "one error".into());
        let errors: Vec<Word> =
            settled(&town, &mut words).await.into_iter().filter(|w| w.kind == ERROR).collect();
        let refused = errors.iter().filter(|e| part(&e.body, "heard") == "mine").count();
        assert_eq!((errors.len(), refused), (3, 2), "the first, and one refusal each: {errors:?}");
        town.reopen();
    });
}

// a resident that fails to open is said once every resident listens, so one listening for
// `town/error` hears it
#[test]
fn a_resident_hears_what_failed_to_open() {
    run(async {
        let dir = scratch();
        let watcher = r#"return react {
            on("town/error", function(w) return event("saw", w.body.from) end)
        }"#;
        // the broken first, so the failure is heard only if it is said after all have come
        std::fs::write(dir.join("a-broken.lua"), "return {").unwrap();
        std::fs::write(dir.join("b-watcher.lua"), watcher).unwrap();
        let (town, mut words) = town(&dir, &[dir.to_path_buf()]);
        assert_eq!(until(&mut words, |w| w.kind == "saw").await.body, "a-broken.lua");
        town.reopen();
    });
}

// a fact said as the town opens is heard once: said as the resident that listens for it comes, it
// comes in its present or as a word, not both
#[test]
fn a_fact_said_as_the_town_opens_is_heard_once() {
    run(async {
        let dir = scratch();
        let r = dir.join("r.lua");
        let file = r#"talk(fact("colors", "bg=#000"))
            return { kinds = { "colors" },
              listen = function(w) return event("heard", w.body) end }"#;
        std::fs::write(&r, file).unwrap();
        let (town, mut words) = town(&dir, std::slice::from_ref(&r));
        assert_eq!(until(&mut words, |w| w.kind == "heard").await.body, "bg=#000");
        // one after: were the first heard twice, the next would be it again
        town.talk(Word::new("colors", Tense::Present, Body::of("bg=#111")));
        assert_eq!(until(&mut words, |w| w.kind == "heard").await.body, "bg=#111");
        town.reopen();
    });
}

// a resident speaks as itself, whenever it talks: as it opens, and later from outside its own
// hearing, from a function it left or a coroutine it began, called in another's. So it may say
// what it owns.
#[test]
fn a_resident_always_speaks_as_itself() {
    run(async {
        let dir = scratch();
        let owner = r#"talk(event("opened"))
            -- given to every file by `_G`: what a resident defines is otherwise its own
            _G.later = coroutine.wrap(function()
              talk(event("mine", "once")); coroutine.yield()
              talk(event("mine", "again"))
            end)
            function _G.mine(said) talk(event("mine", said)) end
            return { own = { "mine" } }"#;
        let caller =
            r#"return react { on("ping", function() later(); later(); mine("left") end) }"#;
        let (a, b) = (dir.join("a-owner.lua"), dir.join("b-caller.lua"));
        std::fs::write(&a, owner).unwrap();
        std::fs::write(&b, caller).unwrap();
        let (town, mut words) = town(&dir, &[a.clone(), b.clone()]);
        let opened = until(&mut words, |w| w.kind == "opened").await;
        came(&mut words, &b).await;
        let id = id(&town, &a).unwrap();
        assert_eq!(opened.by, id, "as it opens");
        town.talk(said("ping", Tense::Past));
        for said in ["once", "again", "left"] {
            let w = until(&mut words, |w| w.kind == "mine" || w.kind == ERROR).await;
            assert_eq!((w.kind.as_str(), w.by), ("mine", id), "{said}: {:?}", w.body);
            assert_eq!(w.body, said);
        }
        town.reopen();
    });
}

// a file of Lua does not hear its own error, though it hears `town/error`
#[test]
fn a_file_of_lua_does_not_hear_its_own_error() {
    run(async {
        let dir = scratch();
        let file = r#"return react {
            on("ping", function() error("no") end),
            on("town/error", function() return event("saw") end),
        }"#;
        let a = dir.join("a.lua");
        std::fs::write(&a, file).unwrap();
        let (town, mut words) = town(&dir, std::slice::from_ref(&a));
        came(&mut words, &a).await;
        town.talk(said("ping", Tense::Past));
        let said = settled(&town, &mut words).await;
        assert!(said.iter().any(|w| w.kind == ERROR), "it failed");
        assert!(!said.iter().any(|w| w.kind == "saw"), "and did not hear it: {said:?}");
        town.reopen();
    });
}

// a file of Lua speaks for none, as its words name no `by`: one it says under `town/residents/`,
// talked as it is read, keeps it from coming, and it owns nothing
#[test]
fn a_file_of_lua_speaks_for_none() {
    run(async {
        let dir = scratch();
        let says = r#"fact("town/residents/sub", { own = { "theme" } })"#;
        let a = dir.join("a.lua");
        std::fs::write(&a, format!("talk({says}); return {{}}")).unwrap();
        let (town, mut words) = town(&dir, std::slice::from_ref(&a));
        let e = until(&mut words, |w| w.kind == ERROR).await;
        assert!(message(&e).contains("speaks for none"), "{}", message(&e));
        assert!(id(&town, &a).is_none(), "it did not come");
        let theme = said("theme", Tense::Past);
        assert!(town.may(&theme, &town.who("other", false)).is_ok(), "it owns nothing");
        town.reopen();
    });
}

// a library is words, not a place to keep things: each resident runs it in its own place, so what
// it defines, and what a library keeps, is that resident's own, unseen by the others
#[test]
fn a_residents_names_and_its_librarys_are_its_own() {
    run(async {
        let dir = scratch();
        let lib = "count = 0; function bump() count = count + 1; return count end";
        let a = r#"secret = "a's"; talk(event("bumped", bump())); return { own = { "a" } }"#;
        let b = r#"talk(event("seen", tostring(secret))); talk(event("count", count))
            talk(event("bumped", bump())); return { own = { "b" } }"#;
        let files = [("a-lib.lua", lib), ("b-a.lua", a), ("c-b.lua", b)];
        for (name, src) in files {
            std::fs::write(dir.join(name), src).unwrap();
        }
        let (town, mut words) = town(&dir, &files.map(|(name, _)| dir.join(name)));
        let said = |kind: &'static str| move |w: &Word| w.kind == kind;
        assert_eq!(until(&mut words, said("bumped")).await.body, Body::of(1i64));
        assert_eq!(until(&mut words, said("seen")).await.body, "nil", "a's own, unseen by b");
        assert_eq!(until(&mut words, said("count")).await.body, Body::of(0i64), "b's own copy");
        assert_eq!(until(&mut words, said("bumped")).await.body, Body::of(1i64), "its own count");
        town.reopen();
    });
}

// a resident that hears every kind hears the whole present as it opens
#[test]
fn every_kind_hears_the_whole_present() {
    run(async {
        let dir = scratch();
        let r = dir.join("r.lua");
        let file = r#"return react{ on({ tense = "present" }, function(w)
            return event("heard", w.kind) end) }"#;
        std::fs::write(&r, file).unwrap();
        let town = Town::with_log(dir.log());
        town.talk(Word::new("theme", Tense::Present, Body::of("dark")));
        let mut words = town.words.listen();
        town::residents::keep(&town, vec![lua(), r]);
        assert_eq!(
            until(&mut words, |w| w.kind == "heard" && w.body == "theme").await.body,
            "theme"
        );
        town.reopen();
    });
}

// kinds that nest hear a word once: `/nvim` and `nvim/1` both hear `nvim/1`
#[test]
fn nested_kinds_hear_a_word_once() {
    run(async {
        let dir = scratch();
        let r = dir.join("r.lua");
        let file = r#"return { kinds = { "/nvim", "nvim/1" }, listen = function(w)
            return { kind = "heard", tense = "past", body = w.kind } end }"#;
        std::fs::write(&r, file).unwrap();
        let (town, mut words) = town(&dir, std::slice::from_ref(&r));
        came(&mut words, &r).await;
        for kind in ["nvim/1", "nvim/2"] {
            town.talk(said(kind, Tense::Past));
        }
        let mut next = async || until(&mut words, |w| w.kind == "heard").await.body;
        assert_eq!((next().await, next().await), (Body::of("nvim/1"), Body::of("nvim/2")));
        town.reopen();
    });
}

// a file gives names another town may come by, each with its kinds: a list, though empty
#[test]
fn a_file_gives_a_name_another_town_may_come_by() {
    run(async {
        let dir = scratch();
        let n = dir.join("n.lua");
        let file = r#"return react { names = { luke = { kinds = { "hi" } }, none = { kinds = {} } },
            on("ping", function() return event("pong") end) }"#;
        std::fs::write(&n, file).unwrap();
        let town = Town::with_log(dir.log());
        let mut words = town.words.listen();
        town::residents::keep(&town, vec![lua(), n.clone()]);
        let kind = fact(&n);
        let here = until(&mut words, move |w| w.kind == kind && !w.body.is_none()).await;
        let names = here.body.part("names").and_then(|n| n.get("luke")).cloned();
        assert_eq!(names, Some(Parts::map([("kinds", Parts::List(vec!["hi".into()]))])));
        let none = here.body.part("names").and_then(|n| n.get("none")).cloned();
        assert_eq!(none, Some(Parts::map([("kinds", Parts::List(vec![]))])), "none is a list");
    });
}

// a name is its kinds and those it excepts, and nothing else: one misspelled, or no table, is
// refused, not given empty
#[test]
fn a_name_is_its_kinds() {
    run(async {
        let dir = scratch();
        let refused = [
            ("kind.lua", r#"{ luke = { kind = { "x" } } }"#, "`luke`: a name is { kinds, except }"),
            ("text.lua", r#"{ luke = "x" }"#, "`luke`: a name is { kinds, except }"),
            ("list.lua", r#"{ "luke" }"#, "each a name and its { kinds, except }"),
        ];
        for (file, names, why) in refused {
            let path = dir.join(file);
            std::fs::write(&path, format!("return {{ names = {names} }}")).unwrap();
            let (_town, mut words) = town(&dir, &[path]);
            let e = until(&mut words, |w| w.kind == ERROR).await;
            assert!(message(&e).contains(why), "{}", message(&e));
        }
    });
}

// a file saved alone says each fact it has, as it is read and as it answers its present, then
// `now`: what it said before and did not again goes, and what it says to its present stays
#[test]
fn a_file_saved_alone_holds_what_it_says_again() {
    run(async {
        let dir = scratch();
        let r = dir.join("r.lua");
        let file = |says: &str| {
            let to = r#"on({ kind = "w", tense = "present" }, function(w)
                return fact("c", w.body) end)"#;
            format!(r#"{says} return react {{ own = {{ "a", "b", "c" }}, {to} }}"#)
        };
        std::fs::write(&r, file(r#"talk(fact("a", 1)) talk(fact("b", 1))"#)).unwrap();
        let town = Town::with_log(dir.log());
        town.talk(Word::new("w", Tense::Present, Body::of(5i64)));
        let mut words = town.words.listen();
        town::residents::keep(&town, probed(&dir, std::slice::from_ref(&r)));
        until(&mut words, |w| w.kind == "c").await;
        settled(&town, &mut words).await;
        std::fs::write(&r, file(r#"talk(fact("a", 2))"#)).unwrap();
        saved(&town, &r);
        until(&mut words, |w| w.kind == "a" && w.body == Body::of(2i64)).await;
        settled(&town, &mut words).await;
        let fact = |k: &str| town.facts(k).remove(k);
        assert_eq!(fact("a"), Some(Body::of(2i64)), "said again");
        assert_eq!(fact("b"), None, "not said again: gone");
        assert_eq!(fact("c"), Some(Body::of(5i64)), "said to its present: stays");
    });
}

// a file reads what town keeps of any kind: its fact now, or its facts before, oldest first
#[test]
fn a_file_reads_what_town_keeps() {
    run(async {
        let dir = scratch();
        let r = dir.join("r.lua");
        let file = r#"return react {
            on("ping", function()
              local under = 0; for _ in pairs(present("weather/")) do under = under + 1 end
              return event("read", { weather = present("weather"), under = under,
                before = past("weather"), none = past("none") })
            end) }"#;
        std::fs::write(&r, file).unwrap();
        let town = Town::with_log(dir.log());
        for weather in ["cloud", "rain"] {
            town.talk(Word::new("weather", Tense::Present, Body::of(weather)));
        }
        for k in ["weather/1", "weather/2"] {
            town.talk(Word::new(k, Tense::Present, Body::of(1i64)));
        }
        let mut words = town.words.listen();
        town::residents::keep(&town, vec![lua(), r.clone()]);
        came(&mut words, &r).await;
        town.talk(said("ping", Tense::Past));
        let read = until(&mut words, |w| w.kind == "read").await;
        assert_eq!(part(&read.body, "weather"), "rain", "its present");
        assert_eq!(part(&read.body, "under"), "2", "a pattern's, by kind");
        let before = read.body.part("before").cloned().unwrap_or_default();
        assert_eq!(
            before,
            Parts::List(vec!["cloud".into(), "rain".into()]),
            "its past, oldest first"
        );
        assert_eq!(read.body.part("none").cloned().unwrap_or_default(), Parts::List(vec![]));
        town.reopen();
    });
}

// a resident joins as many as it likes, each a kind under one, and one clause hears them all, each
// by its kind: here two programs, each saying who it is, then staying
#[test]
fn a_resident_hears_each_join_under_its_kind() {
    run(async {
        let dir = scratch();
        let peers = dir.join("peers.lua");
        let file = r#"local said = seq{ { tense = be("", "past") },
                 { body = seq{ { said = upto("\n") } } } }
               for _, name in ipairs({ "a", "b" }) do
                 local peer = { "sh", "-c", "echo from " .. name .. "; exec cat" }
                 talk(join("peer/" .. name, peer, { speaks = "said" }))
               end
               return react { languages = { said = said },
                 on({ kind = "peer/", tense = "past", by = "said" }, function(w)
                   return event("heard", { from = w.kind, said = w.body.said })
                 end) }"#;
        std::fs::write(&peers, file).unwrap();
        let (town, mut words) = town(&dir, std::slice::from_ref(&peers));
        let mut heard = Vec::new();
        for _ in 0..2 {
            let w = until(&mut words, |w| w.kind == "heard").await;
            heard.push((part(&w.body, "from"), part(&w.body, "said")));
        }
        heard.sort();
        let want = [("peer/a", "from a"), ("peer/b", "from b")].map(|(k, s)| (k.into(), s.into()));
        assert_eq!(heard, want);
        town.reopen();
    });
}

// a resident is its kinds, listen, own and languages, and nothing else: one with a field it does
// not have, such as `watch`, is told so, and does not come; and one that cannot come owns nothing
#[test]
fn a_resident_is_only_what_it_can_be() {
    run(async {
        let dir = scratch();
        let (old, bad) = (dir.join("old.lua"), dir.join("bad.lua"));
        std::fs::write(&old, r#"return { kinds = { "x" }, watch = { colors = "colors" } }"#)
            .unwrap();
        std::fs::write(&bad, r#"return { kinds = { "x" }, listen = "oops", own = { "noted" } }"#)
            .unwrap();
        let (town, mut words) = town(&dir, &[old.clone(), bad.clone()]);
        let e = until(&mut words, |w| w.kind == ERROR).await;
        assert!(message(&e).contains("a resident has no `watch`"), "{}", message(&e));
        settled(&town, &mut words).await;
        assert!(id(&town, &old).is_none() && id(&town, &bad).is_none(), "neither came");
        let noted = said("noted", Tense::Past);
        assert!(town.may(&noted, &town.who("square", true)).is_ok(), "none owns `noted`");
        town.reopen();
    });
}

// the files `lua` speaks for: a save opens that one again alone, the others as they were; one
// deleted goes; a library, which the files after it are written with, opens them all again; and
// as `lua` goes, they go with it, and what they owned is held for them
#[test]
fn a_save_opens_one_file_again_alone() {
    run(async {
        let dir = scratch();
        let (lib, a, b) = (dir.join("0-lib.lua"), dir.join("a.lua"), dir.join("b.lua"));
        std::fs::write(&lib, "function mine(k) return { k } end").unwrap();
        std::fs::write(&a, r#"return { own = mine("a") }"#).unwrap();
        std::fs::write(&b, r#"return { own = mine("b") }"#).unwrap();
        let (town, mut words) = town(&dir, &[dir.to_path_buf()]);
        came(&mut words, &a).await;
        came(&mut words, &b).await;
        let (was_a, was_b) = (id(&town, &a), id(&town, &b));

        saved(&town, &a);
        came(&mut words, &a).await;
        assert!(id(&town, &a) != was_a, "it comes again");
        assert_eq!(id(&town, &b), was_b, "alone: the others stay");

        std::fs::remove_file(&a).unwrap();
        saved(&town, &a);
        let kind = fact(&a);
        until(&mut words, move |w| w.kind == kind && w.body.is_none()).await;
        assert_eq!(id(&town, &b), was_b, "one deleted goes, alone");

        saved(&town, &lib);
        let kind = fact(&b);
        until(&mut words, move |w| w.kind == kind && w.body.is_none()).await;
        came(&mut words, &b).await;
        assert!(id(&town, &b) != was_b, "a library opens them all again");

        town.reopen();
        let kind = fact(&b);
        until(&mut words, move |w| w.kind == kind && w.body.is_none()).await;
        assert!(id(&town, &b).is_none(), "gone with lua");
        // what it owned is held for it to come again: another may not say it meanwhile
        let theirs = said("b", Tense::Past);
        let why = town.may(&theirs, &town.who("other", false)).unwrap_err();
        assert!(why.contains("held"), "what it owned is held: {why}");
    });
}

// a save of two files, one a library now, opens all again once: none of what came before is left
// running beside what comes after
#[test]
fn a_save_that_makes_a_library_leaves_none_running_twice() {
    run(async {
        let dir = scratch();
        let (a, b) = (dir.join("a.lua"), dir.join("b.lua"));
        std::fs::write(&a, r#"return { own = { "a" } }"#).unwrap();
        let pongs = r#"return react { on("ping", function() return event("pong") end) }"#;
        std::fs::write(&b, pongs).unwrap();
        let (town, mut words) = town(&dir, &[dir.to_path_buf()]);
        came(&mut words, &b).await;
        std::fs::write(&a, "function helps() end").unwrap();
        saved(&town, &a);
        saved(&town, &b);
        let kind = fact(&b);
        until(&mut words, move |w| w.kind == kind && w.body.is_none()).await;
        came(&mut words, &b).await;
        settled(&town, &mut words).await;
        town.talk(said("ping", Tense::Past));
        let pongs =
            settled(&town, &mut words).await.into_iter().filter(|w| w.kind == "pong").count();
        assert_eq!(pongs, 1, "one b");
        town.reopen();
    });
}

/// a town that speaks town's own languages, as any town does: `lua` speaking for them
async fn speaking() -> (Scratch, Rc<Town>) {
    let dir = scratch();
    let languages = Path::new(env!("CARGO_MANIFEST_DIR")).join("residents/languages");
    let (town, mut words) = town(&dir, std::slice::from_ref(&languages));
    for name in ["cbor", "kv", "line", "msgpack", "words"] {
        came(&mut words, &languages.join(format!("{name}.lua"))).await;
    }
    (dir, town)
}

/// a town that speaks the language `name`, of `grammar`: a resident that is it
async fn speaking_one(name: &str, grammar: &str) -> (Scratch, Rc<Town>) {
    let dir = scratch();
    let file = dir.join(format!("{name}.lua"));
    std::fs::write(&file, format!("return {{ languages = {{ {name} = {grammar} }} }}")).unwrap();
    let (town, mut words) = town(&dir, std::slice::from_ref(&file));
    came(&mut words, &file).await;
    (dir, town)
}

/// what a language makes of these parts, and what it reads back
fn both(town: &Town, language: &str, json: serde_json::Value, bytes: &[u8]) {
    let parts = Parts::from(json);
    assert_eq!(
        town.languages.bytes(language, &parts).as_deref(),
        Ok(bytes),
        "{language}: {parts:?}"
    );
    assert_eq!(town.languages.parts(language, bytes), Ok(parts), "{language}: {bytes:02x?}");
}

// a word on a line in `words`: its tense, if said, its kind, its body's key=value; written back as
// it was read
#[test]
fn words_are_a_word_on_a_line() {
    run(async {
        let (_dir, town) = speaking().await;
        let read = town.languages.parts("words", b"past\tshout\ttext=hi there\tn=\n").unwrap();
        let w = town::voice::word_at("", &read).unwrap();
        assert_eq!((w.kind.as_str(), w.tense), ("shout", Tense::Past));
        assert_eq!(w.body, Parts::from(serde_json::json!({ "text": "hi there", "n": "" })));
        let read = town.languages.parts("words", b"pastry\n").unwrap();
        let w = town::voice::word_at("", &read).unwrap();
        assert_eq!((w.kind.as_str(), w.tense), ("pastry", Tense::Present));
        let bare = &b"present\tnvim\n"[..];
        let read = town.languages.parts("words", bare).unwrap();
        assert_eq!(read.get("body").cloned().unwrap_or_default(), Parts::None, "{read:?}");
        let words = town.languages.get("words").unwrap();
        let none = Word::new("nvim", Tense::Present, Body::none());
        assert_eq!(town::voice::bytes(&none, None, &words, &town.languages).unwrap(), bare);
        let now = town::voice::bytes(&Word::now(), None, &words, &town.languages).unwrap();
        assert_eq!(now, b"past\t\n");
        assert!(town.languages.parts("words", b"\n").is_err(), "a blank line is no word");
        town.reopen();
    });
}

// a map whose entries are each written by a map, of the pair: a map that is not its own, so it is
// no loop, and writes. A map given back the pair it made is the loop, and refused
#[test]
fn a_map_of_maps_writes() {
    run(async {
        let pair = r#"map(seq{ { key = upto(":") }, { value = upto("\n") } })"#;
        let (_dir, town) = speaking_one("mm", &format!("map({pair})")).await;
        let parts = Parts::from(serde_json::json!({ "a": "1" }));
        let bytes = town.languages.bytes("mm", &parts);
        assert_eq!(bytes.as_deref(), Ok(&b"key:a\nvalue:1\n"[..]), "each pair, as a map");
        town.reopen();
    });
}

// msgpack, each value in its smallest form, and read back as it was
#[test]
fn msgpack_is_a_grammar() {
    run(async {
        use serde_json::json;
        let (_dir, town) = speaking().await;
        both(&town, "msgpack", json!(5), b"\x05");
        both(&town, "msgpack", json!(-1), b"\xff");
        both(&town, "msgpack", json!(300), b"\xcd\x01\x2c");
        both(&town, "msgpack", json!(-100), b"\xd0\x9c");
        both(&town, "msgpack", json!(1.5), b"\xca\x3f\xc0\x00\x00");
        both(&town, "msgpack", json!(0.1), b"\xcb\x3f\xb9\x99\x99\x99\x99\x99\x9a");
        both(&town, "msgpack", json!(null), b"\xc0");
        both(&town, "msgpack", json!([]), b"\x90");
        both(
            &town,
            "msgpack",
            json!({ "a": 1, "b": [true, null, "x"] }),
            b"\x82\xa1a\x01\xa1b\x93\xc3\xc0\xa1x",
        );
        let long = "x".repeat(40);
        let mut bytes = b"\xd9\x28".to_vec();
        bytes.extend(long.as_bytes());
        both(&town, "msgpack", json!(long), &bytes);
        let raw = Parts::Bytes(vec![1, 2]);
        assert_eq!(town.languages.bytes("msgpack", &raw).unwrap(), b"\xc4\x02\x01\x02");
        assert!(town.languages.parts("msgpack", b"\x92\x01").is_err(), "half an array");
        let ext = |n: usize| {
            let data = vec![7u8; n];
            Parts::map([("ext", Parts::from(5i64)), ("data", Parts::Bytes(data))])
        };
        for n in [1, 3, 256] {
            let bytes = town.languages.bytes("msgpack", &ext(n)).unwrap();
            assert_eq!(town.languages.parts("msgpack", &bytes).unwrap(), ext(n), "an ext of {n}");
        }
        town.reopen();
    });
}

// cbor, as RFC 8949's examples say it, and in pieces till a break, read as the one value it is
#[test]
fn cbor_is_a_grammar() {
    run(async {
        use serde_json::json;
        let (_dir, town) = speaking().await;
        for (n, bytes) in [
            (json!(0), &b"\x00"[..]),
            (json!(23), b"\x17"),
            (json!(24), b"\x18\x18"),
            (json!(1000), b"\x19\x03\xe8"),
            (json!(1000000), b"\x1a\x00\x0f\x42\x40"),
            (json!(18446744073709551615u64), b"\x1b\xff\xff\xff\xff\xff\xff\xff\xff"),
            (json!(-1), b"\x20"),
            (json!(-1000), b"\x39\x03\xe7"),
            (json!(1.5), b"\xf9\x3e\x00"),
            (json!(1.1), b"\xfb\x3f\xf1\x99\x99\x99\x99\x99\x9a"),
            (json!(false), b"\xf4"),
            (json!(null), b"\xf6"),
            (json!("\u{00fc}"), b"\x62\xc3\xbc"),
            (json!([1, [2, 3]]), b"\x82\x01\x82\x02\x03"),
            (json!({ "a": 1, "b": [2, 3] }), b"\xa2\x61a\x01\x61b\x82\x02\x03"),
        ] {
            both(&town, "cbor", n, bytes);
        }
        let read = |b: &[u8]| town.languages.parts("cbor", b).unwrap();
        assert_eq!(read(b"\x9f\x01\x82\x02\x03\xff"), Parts::from(json!([1, [2, 3]])));
        assert_eq!(read(b"\x7f\x65strea\x64ming\xff"), Parts::from("streaming"));
        let nulls = Parts::List(vec![1i64.into(), Parts::None, 2i64.into()]);
        assert_eq!(read(b"\x9f\x01\xf6\x02\xff"), nulls, "a null keeps its place");
        let tagged = read(b"\xc1\x1a\x51\x4b\x67\xb0");
        assert_eq!(tagged, Parts::from(json!({ "tag": 1, "value": 1363896240 })));
        town.reopen();
    });
}
// a string named in a grammar is those bytes, kept as the part of that name; and a part may be
// named for a piece, and is still a part
#[test]
fn a_named_string_is_kept() {
    run(async {
        let (_dir, town) =
            speaking_one("mark", r#"seq{ { is = "w" }, "~", { name = but("|") } }"#).await;
        let read = town.languages.parts("mark", b"w~alpha").unwrap();
        assert_eq!(read, Parts::from(serde_json::json!({ "is": "w", "name": "alpha" })));
        assert!(town.languages.parts("mark", b"p~alpha").is_err());
        town.reopen();
        let (_dir, town) =
            speaking_one("note", r#"seq{ { text = upto("\n") }, { map = but("\n") } }"#).await;
        let note = town.languages.parts("note", b"hi\nthere").unwrap();
        assert_eq!(note, Parts::from(serde_json::json!({ "text": "hi", "map": "there" })));
        town.reopen();
    });
}

// a language that begins with itself ends: where it is already being read, it reads nothing
#[test]
fn a_language_may_name_itself() {
    run(async {
        let grammar = r#"choice{ seq{ ref("x"), "a" }, seq{ ref("x"), "b" }, "c" }"#;
        let (_dir, town) = speaking_one("x", grammar).await;
        assert!(town.languages.parts("x", b"z").is_err());
        assert_eq!(town.languages.parts("x", b"c"), Ok(Parts::from("c")));
        assert!(town.languages.bytes("x", &Parts::from("z")).is_err());
        town.reopen();
    });
}

// what a library talks as it is read is said once, by lua, as the library is read alone; read into
// each resident after it, it says nothing again
#[test]
fn a_library_says_its_words_once() {
    run(async {
        let dir = scratch();
        std::fs::write(dir.join("0-lib.lua"), r#"talk(event("opened"))"#).unwrap();
        for name in ["a", "b"] {
            std::fs::write(
                dir.join(format!("{name}.lua")),
                format!(r#"return {{ own = {{ "{name}" }} }}"#),
            )
            .unwrap();
        }
        let town = Town::with_log(dir.log());
        let mut words = town.words.listen();
        let mut opened = town.words.hear(|_, w| w.kind == "opened", None);
        town::residents::keep(&town, probed(&dir, &[dir.to_path_buf()]));
        came(&mut words, &dir.join("b.lua")).await;
        settled(&town, &mut words).await;
        let opened = std::iter::from_fn(|| opened.try_recv().ok()).count();
        assert_eq!(opened, 1, "once");
        town.reopen();
    });
}

// a file that hears every kind but those it `except`s, what is under `x`, what it says, and the
// probe's, hears `y` and not `x/1`. `x/1`, said first, would be heard first
#[test]
fn a_resident_hears_its_kinds_but_those_it_excepts() {
    run(async {
        let dir = scratch();
        let a = dir.join("a.lua");
        let file = r#"return { kinds = { "/" }, except = { "x/", "heard", "probe", "probed" },
            listen = function(w) return event("heard", { kind = w.kind }) end }"#;
        std::fs::write(&a, file).unwrap();
        let (town, mut words) = town(&dir, std::slice::from_ref(&a));
        came(&mut words, &a).await;
        settled(&town, &mut words).await;
        town.talk(said("x/1", Tense::Past));
        town.talk(said("y", Tense::Past));
        let heard = until(&mut words, |w| w.kind == "heard").await;
        assert_eq!(part(&heard.body, "kind"), "y", "not what it excepts");
        town.reopen();
    });
}

// a resident saved as a library says what it talks as it is read once: read as the save asks it,
// it is read again with the files after it, and only that read is said
#[test]
fn a_resident_saved_as_a_library_says_its_words_once() {
    run(async {
        let dir = scratch();
        let (a, b) = (dir.join("a.lua"), dir.join("b.lua"));
        std::fs::write(&a, r#"return { own = { "a" } }"#).unwrap();
        std::fs::write(&b, r#"return { own = { "b" } }"#).unwrap();
        let (town, mut words) = town(&dir, &[dir.to_path_buf()]);
        came(&mut words, &b).await;
        settled(&town, &mut words).await;
        std::fs::write(&a, r#"talk(event("opened"))"#).unwrap();
        saved(&town, &a);
        let said = settled(&town, &mut words).await;
        assert!(id(&town, &b).is_some(), "b came again with it");
        assert_eq!(said.iter().filter(|w| w.kind == "opened").count(), 1, "once: {said:?}");
        town.reopen();
    });
}

// a save of a file as it was, the version lua read, does nothing: an ask is no change
#[test]
fn a_file_asked_as_it_was_stays() {
    run(async {
        let dir = scratch();
        let a = dir.join("a.lua");
        std::fs::write(&a, r#"return { own = { "a" } }"#).unwrap();
        let (town, mut words) = town(&dir, std::slice::from_ref(&a));
        came(&mut words, &a).await;
        let was = id(&town, &a);
        let path = Parts::map([("path", a.to_string_lossy().as_ref())]);
        town.talk(Word::new("town/open", Tense::Future, Body::of(path)));
        let said = settled(&town, &mut words).await;
        let kind = fact(&a);
        let went = said.iter().any(|w| w.kind == kind && w.tense == Tense::Present);
        assert!(!went, "it stays: {said:?}");
        assert_eq!(id(&town, &a), was);
        town.reopen();
    });
}

// a file is told its present once as it comes again: what lua heard before, lua tells it; what
// it hears now that lua did not, town does
#[test]
fn a_file_is_told_its_present_once() {
    run(async {
        let dir = scratch();
        let a = dir.join("a.lua");
        let file = r#"return react { on("x", function(w) return event("told", w.body) end) }"#;
        std::fs::write(&a, file).unwrap();
        let town = Town::with_log(dir.log());
        town.talk(Word::new("x", Tense::Present, Body::of("one")));
        let mut words = town.words.listen();
        let mut told = town.words.hear(|_, w| w.kind == "told", None);
        town::residents::keep(&town, probed(&dir, std::slice::from_ref(&a)));
        came(&mut words, &a).await;
        saved(&town, &a);
        let kind = fact(&a);
        until(&mut words, move |w| w.kind == kind && w.body.is_none()).await;
        came(&mut words, &a).await;
        settled(&town, &mut words).await;
        let told = std::iter::from_fn(|| told.try_recv().ok()).count();
        assert_eq!(told, 2, "once as it first came, once as it came again");
        town.reopen();
    });
}

// at town's start, a file may read and write a language a file before it is
#[test]
fn a_file_speaks_a_language_a_file_before_it_is() {
    run(async {
        let dir = scratch();
        std::fs::write(dir.join("a.lua"), r#"return { languages = { zz = upto("\n") } }"#).unwrap();
        let b = dir.join("b.lua");
        std::fs::write(&b, r#"talk(event("read", parts("zz", "hi\n"))); return { own = { "b" } }"#)
            .unwrap();
        let (town, mut words) = town(&dir, &[dir.join("a.lua"), b.clone()]);
        assert_eq!(until(&mut words, |w| w.kind == "read" || w.kind == ERROR).await.body, "hi");
        town.reopen();
    });
}

// a file hears town's errors of the other files `lua` speaks for, though not its own: a word one
// says that another owns is refused, and a third that hears `town/error` hears it
#[test]
fn a_file_hears_the_errors_of_the_others() {
    run(async {
        let dir = scratch();
        let says = r#"return react { on("ping", function() return event("mine") end) }"#;
        std::fs::write(dir.join("a.lua"), says).unwrap();
        let hears = r#"return react {
            on("town/error", function(w) return event("saw", w.body.from) end) }"#;
        std::fs::write(dir.join("b.lua"), hears).unwrap();
        std::fs::write(dir.join("c.lua"), r#"return { own = { "mine" } }"#).unwrap();
        let (town, mut words) = town(&dir, &[dir.to_path_buf()]);
        came(&mut words, &dir.join("c.lua")).await;
        town.talk(said("ping", Tense::Past));
        assert_eq!(until(&mut words, |w| w.kind == "saw").await.body, "a.lua");
        town.reopen();
    });
}

// a language is a resident that is it: another that says it is one of the name is refused, and
// the first is spoken still; and `lua` drops the one refused, so what it would say is not said,
// and it errs no more
#[test]
fn a_language_is_one_resident_and_the_one_refused_is_dropped() {
    run(async {
        let dir = scratch();
        let x = |g: &str| {
            let pongs = r#"on("ping", function() return event("pong") end)"#;
            format!("return react {{ languages = {{ x = {g} }}, {pongs} }}")
        };
        std::fs::write(dir.join("a.lua"), x(r#"upto("\n")"#)).unwrap();
        std::fs::write(dir.join("b.lua"), x(r#"but("\n")"#)).unwrap();
        let (town, mut words) = town(&dir, &[dir.join("a.lua"), dir.join("b.lua")]);
        let refused = until(&mut words, |w| w.kind == ERROR).await;
        assert!(message(&refused).contains("another is `x`"), "{}", message(&refused));
        assert_eq!(town.languages.parts("x", b"hi\n"), Ok(Parts::from("hi")), "a's, still");
        settled(&town, &mut words).await;
        town.talk(said("ping", Tense::Past));
        let said = settled(&town, &mut words).await;
        assert_eq!(said.iter().filter(|w| w.kind == "pong").count(), 1, "a's alone: {said:?}");
        assert!(!said.iter().any(|w| w.kind == ERROR), "b errs no more: {said:?}");
        town.reopen();
    });
}

// a file saved alone begins again: it keeps no names across an open, and has only the libraries
// read before it first came, never one after it; and one deleted goes quietly, with no error
#[test]
fn a_file_saved_alone_begins_again_with_the_libraries_before_it() {
    run(async {
        let dir = scratch();
        let (lib, b, late) = (dir.join("0-lib.lua"), dir.join("b.lua"), dir.join("c-lib.lua"));
        std::fs::write(&lib, r#"function first() return "first" end"#).unwrap();
        let counts = r#"n = (n or 0) + 1
            talk(event("counted", { n = n, first = first(), late = tostring(late) }))
            return { own = { "b" } }"#;
        std::fs::write(&b, counts).unwrap();
        std::fs::write(&late, r#"late = "after""#).unwrap();
        let town = Town::with_log(dir.log());
        let mut words = town.words.listen();
        let mut counted = town.words.hear(|_, w| w.kind == "counted", None);
        town::residents::keep(&town, probed(&dir, &[dir.to_path_buf()]));
        came(&mut words, &b).await;
        saved(&town, &b);
        let kind = fact(&b);
        until(&mut words, move |w| w.kind == kind && w.body.is_none()).await;
        came(&mut words, &b).await;
        settled(&town, &mut words).await;
        let all: Vec<Word> = std::iter::from_fn(|| counted.try_recv().ok()).collect();
        assert_eq!(all.len(), 2, "as it first came, and as it came again");
        for w in &all {
            assert_eq!(part(&w.body, "n"), "1", "it begins again");
            assert_eq!(part(&w.body, "first"), "first", "the library before it");
            assert_eq!(part(&w.body, "late"), "nil", "never the one after it");
        }

        let mut errors = town.words.hear(|_, w| w.kind == ERROR, None);
        std::fs::remove_file(&b).unwrap();
        saved(&town, &b);
        let kind = fact(&b);
        until(&mut words, move |w| w.kind == kind && w.body.is_none()).await;
        settled(&town, &mut words).await;
        assert!(errors.try_recv().is_err(), "deleted, it goes quietly");
        town.reopen();
    });
}

// a file saved alone hears a fact once, though it says it as it is read: the fact, said while it
// reads its present, is in what it reads, and not told it again
#[test]
fn a_file_saved_alone_hears_what_it_says_as_it_is_read_once() {
    run(async {
        let dir = scratch();
        let (a, b) = (dir.join("a.lua"), dir.join("b.lua"));
        let says = r#"talk(fact("x", "a"))
            return react { on("x", function(w) return event("heard", w.body) end) }"#;
        std::fs::write(&a, says).unwrap();
        std::fs::write(&b, r#"return react { on("x", function() end) }"#).unwrap();
        let (town, mut words) = town(&dir, &[dir.to_path_buf()]);
        came(&mut words, &b).await;
        settled(&town, &mut words).await;
        saved(&town, &a);
        let kind = fact(&a);
        until(&mut words, move |w| w.kind == kind && w.body.is_none()).await;
        let said = settled(&town, &mut words).await;
        assert_eq!(said.iter().filter(|w| w.kind == "heard").count(), 1, "once: {said:?}");
        town.reopen();
    });
}

// a Lua file made a program is `lua`'s no longer: it goes, and town runs it, owning what it owns
#[test]
fn a_lua_file_made_a_program_is_towns() {
    run(async {
        let dir = scratch();
        let a = dir.join("a.lua");
        std::fs::write(&a, r#"return { own = { "a" } }"#).unwrap();
        let (town, mut words) = town(&dir, &[dir.to_path_buf()]);
        came(&mut words, &a).await;
        script(&dir, "a.lua", r#"echo '{"own":["a"]}'; exec cat"#);
        let mut errors = town.words.hear(|_, w| w.kind == ERROR, None);
        saved(&town, &a);
        let kind = fact(&a);
        until(&mut words, move |w| w.kind == kind && w.body.is_none()).await;
        let program = format!("town/residents/{}", trimmed(&a));
        until(&mut words, move |w| w.kind == program && !w.body.is_none()).await;
        settled(&town, &mut words).await;
        assert!(errors.try_recv().is_err(), "it comes as a program, owning what it owns");
        town.reopen();
    });
}

// a file that would be a language another is, refused, leaves the other's spoken in `lua`
#[test]
fn a_language_refused_leaves_the_others_spoken() {
    run(async {
        let dir = scratch();
        let (one, two) = (dir.join("1.lua"), dir.join("2.lua"));
        std::fs::write(&one, r#"return { languages = { ll = upto("\n") } }"#).unwrap();
        let reads = r#"return react {
            on("ping", function() return event("read", parts("ll", "hi\n")) end) }"#;
        std::fs::write(&two, reads).unwrap();
        let (town, mut words) = town(&dir, &[dir.to_path_buf()]);
        came(&mut words, &two).await;
        let claims =
            format!("{}, languages = {{ ll = but(\"\\n\") }} }}", reads.trim_end_matches(" }"));
        std::fs::write(&two, claims).unwrap();
        saved(&town, &two);
        until(&mut words, |w| w.kind == ERROR).await;
        settled(&town, &mut words).await;
        let three = dir.join("3.lua");
        std::fs::write(&three, reads).unwrap();
        saved(&town, &three);
        came(&mut words, &three).await;
        settled(&town, &mut words).await;
        town.talk(said("ping", Tense::Past));
        assert_eq!(until(&mut words, |w| w.kind == "read" || w.kind == ERROR).await.body, "hi");
        town.reopen();
    });
}
