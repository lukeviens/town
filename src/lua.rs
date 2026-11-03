//! Lua's side of town: the Lua residents. A Lua file that returns a resident — { kinds, listen }
//! and perhaps `watch` and `own` — is wired; one that returns nothing is a library, for the files
//! after it. Residents see the town through `present`, `past`, `talk` and the languages, and a word
//! as a table: a part is nil, a string, a number, a boolean, or a table, a list if its keys are 1
//! to n, else parts by name. A nothing in a list is `null`, so the list keeps its places.

use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;

use mlua::{Function, Lua, Table, Value, Value as LuaValue};
use tokio::sync::broadcast::error::RecvError;

use crate::body::Body;
use crate::parts::Parts;
use crate::residents::{hears, named, watch_for, Watched};
use crate::town::Town;
use crate::word::{Tense, Word};

/// Where the mark a list carries is kept.
const LIST: &str = "town.list";

/// parts, as Lua holds them
pub fn lua(lua: &Lua, p: &Parts) -> mlua::Result<Value> {
    Ok(match p {
        Parts::None => Value::Nil,
        Parts::Text(t) => Value::String(lua.create_string(t)?),
        Parts::Bytes(b) => Value::String(lua.create_string(b)?),
        Parts::Number(n) => match (n.parse::<i64>(), n.parse::<f64>()) {
            (Ok(i), _) => Value::Integer(i),
            (_, Ok(f)) => Value::Number(f),
            _ => Value::String(lua.create_string(n)?),
        },
        Parts::Bool(b) => Value::Boolean(*b),
        // a nothing in a list is `null`, so the list keeps its places: a Lua list holds no nil. A
        // list is marked so, so even an empty one is a list again
        Parts::List(l) => {
            let t = lua.create_table_with_capacity(l.len(), 0)?;
            if let Ok(mark) = lua.named_registry_value::<Table>(LIST) {
                t.set_metatable(Some(mark));
            }
            for p in l {
                t.raw_push(match p {
                    Parts::None => Value::NULL,
                    p => self::lua(lua, p)?,
                })?;
            }
            Value::Table(t)
        }
        Parts::Map(m) => {
            let t = lua.create_table_with_capacity(0, m.len())?;
            for (n, p) in m {
                t.raw_set(n.as_str(), self::lua(lua, p)?)?;
            }
            Value::Table(t)
        }
    })
}

/// what Lua holds, as parts
pub fn parts(v: Value) -> mlua::Result<Parts> {
    read(v, 0)
}

fn read(v: Value, deep: usize) -> mlua::Result<Parts> {
    // a table that holds itself goes on without end
    if deep > crate::language::DEEP {
        return Err(mlua::Error::runtime("parts too deep: a table that holds itself?"));
    }
    Ok(match v {
        Value::Nil => Parts::None,
        Value::LightUserData(u) if u.0.is_null() => Parts::None,
        Value::Boolean(b) => b.into(),
        Value::Integer(i) => i.into(),
        Value::Number(n) => n.into(),
        // a string that is not text is bytes
        Value::String(s) => match s.to_str() {
            Ok(t) => t.to_string().into(),
            Err(_) => Parts::Bytes(s.as_bytes().to_vec()),
        },
        Value::Table(t) => {
            let pairs: Vec<(Value, Value)> = t.pairs().collect::<mlua::Result<_>>()?;
            let n = t.raw_len();
            let marked = t.metatable().is_some_and(|m| {
                m.raw_get::<Option<String>>("__name").ok().flatten().as_deref() == Some(LIST)
            });
            if marked || (n > 0 && pairs.len() == n) {
                let l = t.sequence_values().map(|v| read(v?, deep + 1));
                Parts::List(l.collect::<mlua::Result<_>>()?)
            } else {
                let mut m = Parts::Map(Vec::new());
                for (k, v) in pairs {
                    let name = match k {
                        Value::String(s) => s.to_string_lossy(),
                        Value::Integer(i) => i.to_string(),
                        Value::Number(n) => n.to_string(),
                        k => return Err(not_a_part(&k, "name")),
                    };
                    m.set(name, read(v, deep + 1)?);
                }
                m
            }
        }
        v => return Err(not_a_part(&v, "part")),
    })
}

fn not_a_part(v: &Value, what: &str) -> mlua::Error {
    mlua::Error::runtime(format!("a {} is no {what}", v.type_name()))
}

/// a word, as a table: `{ kind, tense, body, when }`
pub fn table(l: &Lua, w: &Word) -> mlua::Result<Table> {
    let t = l.create_table_with_capacity(0, 4)?;
    t.raw_set("kind", w.kind.as_str())?;
    t.raw_set("tense", w.tense.name())?;
    t.raw_set("body", lua(l, w.body.parts())?)?;
    if w.when != 0 {
        t.raw_set("when", w.when)?;
    }
    Ok(t)
}

/// a table, as a word: it has a kind, and a tense if it is not present. Whatever else it holds is
/// the resident's own.
pub fn word(v: Value) -> mlua::Result<Word> {
    let Value::Table(t) = v else { return Err(mlua::Error::runtime("a word is a table")) };
    let Value::String(kind) = t.get("kind")? else {
        return Err(mlua::Error::runtime("a word has a kind, a string"));
    };
    let tense = match t.get::<Value>("tense")? {
        Value::Nil => Tense::Present,
        Value::String(s) => Tense::named(&s.to_str()?)
            .ok_or_else(|| mlua::Error::runtime("a tense is past, present or future"))?,
        _ => return Err(mlua::Error::runtime("a tense is past, present or future")),
    };
    let mut w = Word::new(kind.to_string_lossy(), tense, Body::of(parts(t.get("body")?)?));
    w.when = t.get::<Option<u64>>("when")?.unwrap_or(0);
    Ok(w)
}

thread_local! {
    /// who is listening right now, and to what kind: a failure in their hands is theirs
    static HEARING: RefCell<Option<(String, String)>> = const { RefCell::new(None) };
}

/// A resident's failure, said as an `error` word: who, while hearing what, and why. One that fails
/// while hearing an `error` is only printed: talking it would feed it back to itself.
fn failed(town: &Town, message: String) {
    let hearing = HEARING.with(|h| h.borrow().clone());
    match hearing {
        Some((from, kind)) if kind == "error" => eprintln!("town: {from}: {message}"),
        Some((from, kind)) => town.error(&from, Some(&kind), message),
        None => town.error("talk", None, message),
    }
}

/// Give the residents their windows onto the town: `present`, `past`, and `talk`.
pub fn install(lua: &Lua, town: &Rc<Town>) -> mlua::Result<()> {
    // a nothing in a list, which Lua cannot hold as nil; and a table marked as a list, so even an
    // empty one is said as a list, not as parts by name
    lua.globals().set("null", Value::NULL)?;
    let mark = lua.create_table()?;
    mark.raw_set("__name", LIST)?;
    lua.set_named_registry_value(LIST, mark)?;
    lua.globals().set(
        "list",
        lua.create_function(|lua, t: Option<Table>| {
            let t = match t {
                Some(t) => t,
                None => lua.create_table()?,
            };
            t.set_metatable(Some(lua.named_registry_value::<Table>(LIST)?));
            Ok(t)
        })?,
    )?;

    let now = town.clone();
    lua.globals().set(
        "present",
        lua.create_function(move |lua, kind: String| match now.present.borrow().get(&kind) {
            Some(v) => crate::lua::lua(lua, v.parts()),
            None => Ok(LuaValue::Nil),
        })?,
    )?;

    let then = town.clone();
    lua.globals().set(
        "past",
        lua.create_function(move |lua, kind: String| {
            lua.create_sequence_from(
                then.past(&kind)
                    .iter()
                    .map(|b| crate::lua::lua(lua, b.parts()))
                    .collect::<mlua::Result<Vec<_>>>()?,
            )
        })?,
    )?;

    // every other citizen of town has `talk`; residents could only answer by returning
    let talker = town.clone();
    lua.globals().set(
        "talk",
        lua.create_function(move |_, w: LuaValue| {
            match crate::lua::word(w) {
                Ok(word) => talker.talk(word),
                Err(e) => failed(&talker, format!("talked a non-word: {e}")),
            }
            Ok(())
        })?,
    )?;

    // languages: once said, any may read bytes as parts with one, or write parts as bytes. What
    // cannot be is the asker's error.
    let reads = town.clone();
    lua.globals().set(
        "parts",
        lua.create_function(move |lua, (name, bytes): (String, mlua::String)| {
            let parts = reads.languages.parts(&name, &bytes.as_bytes());
            crate::lua::lua(lua, &parts.map_err(mlua::Error::runtime)?)
        })?,
    )?;
    let writes = town.clone();
    lua.globals().set(
        "bytes",
        lua.create_function(move |lua, (name, parts): (String, LuaValue)| {
            let parts = crate::lua::parts(parts).map_err(|e| wrong(&name, e))?;
            lua.create_string(writes.languages.bytes(&name, &parts).map_err(mlua::Error::runtime)?)
        })?,
    )?;
    Ok(())
}

fn wrong(name: &str, e: impl std::fmt::Display) -> mlua::Error {
    mlua::Error::runtime(format!("{name}: {e}"))
}

pub(crate) fn wire(lua: &Lua, town: &Rc<Town>, path: &Path, age: u64) -> mlua::Result<()> {
    let src = std::fs::read_to_string(path)?;
    let spec: Option<Table> = lua.load(src).set_name(named(path)).eval()?;
    let Some(spec) = spec else {
        return Ok(()); // a library: what it defined is there for the files after it
    };
    // nothing else: a misspelled field, or one a resident no longer has, would do nothing, and say
    // nothing of why
    for key in spec.pairs::<LuaValue, LuaValue>().filter_map(Result::ok).map(|(k, _)| k) {
        let key = key.to_string().unwrap_or_default();
        if !["kinds", "listen", "watch", "own"].contains(&key.as_str()) {
            return Err(mlua::Error::runtime(format!(
                "a resident has no `{key}`: it is `kinds`, `listen`, `watch` and `own`"
            )));
        }
    }
    let from = named(path);

    // watch: files and folders town keeps reading for it
    if let Some(watch) = spec.get::<Option<Table>>("watch")? {
        for pair in watch.pairs::<String, LuaValue>() {
            let (kind, file) = pair?;
            let watched = Watched::of(&parts(file)?).map_err(|e| wrong(&kind, e))?;
            let task = watch_for(town, &from, path, kind, watched);
            town.tasks.borrow_mut().push(task);
        }
    }

    // kinds: each wakes `listen`, and whatever it answers is talked
    let handler: Option<Function> = spec.get("listen")?;
    if let (Some(handler), Some(kinds)) = (handler, spec.get::<Option<Table>>("kinds")?) {
        for kind in kinds.sequence_values::<String>().filter_map(Result::ok) {
            hearing(lua, town, &from, &handler, kind, age);
        }
    }

    // own: kinds this resident alone makes. Refused from the square, so no one outside can say
    // them in its place.
    if let Some(own) = spec.get::<Option<Table>>("own")? {
        for kind in own.sequence_values::<String>().filter_map(Result::ok) {
            town.hold(&kind);
        }
    }
    Ok(())
}

/// One kind a resident hears: the present of it first (all of it, for "*"), then each word of it,
/// until the town opens again. It listens from now, not from when its task first runs, so what
/// opening the town says after this, its failures among them, is heard; and its greeting is the
/// present as of now, so a fact is heard once: said before now, in the greeting; after, as a word.
fn hearing(lua: &Lua, town: &Rc<Town>, from: &str, handler: &Function, kind: String, age: u64) {
    let mut words = town.words.subscribe();
    let greeting: Vec<Word> =
        town.present_words().into_iter().filter(|w| hears(&kind, &w.kind)).collect();
    let (lua, town, from, handler) = (lua.clone(), town.clone(), from.to_string(), handler.clone());
    tokio::task::spawn_local(async move {
        for w in greeting {
            listen(&lua, &handler, &town, &from, &w);
        }
        // the town opening again ends this listening: asked again on waking, since a reopen may
        // come while it waits, and what has ended must not take one last word
        while age >= town.age.get() {
            match words.recv().await {
                Ok(w) if age >= town.age.get() && hears(&kind, &w.kind) => {
                    listen(&lua, &handler, &town, &from, &w)
                }
                Ok(_) | Err(RecvError::Lagged(_)) => {}
                Err(RecvError::Closed) => break,
            }
        }
    });
}

// A resident listens to a word, a table { kind, tense, body, when }, and whatever it answers is
// talked: its tense is present if it says none, and whatever else the table holds (a label, say)
// is its own. Nothing is nothing to say; a Lua error, or an answer that is no word, is said as an
// `error` from it.
fn listen(lua: &Lua, handler: &Function, town: &Town, from: &str, w: &Word) {
    HEARING.with(|h| *h.borrow_mut() = Some((from.to_string(), w.kind.clone())));
    hear(lua, handler, town, w);
    HEARING.with(|h| *h.borrow_mut() = None);
}

fn hear(lua: &Lua, handler: &Function, town: &Town, w: &Word) {
    let fail = |message: String| failed(town, message);
    let heard = match crate::lua::table(lua, w) {
        Ok(t) => t,
        Err(e) => return fail(e.to_string()),
    };
    let said = match handler.call::<LuaValue>(heard) {
        Ok(v) => v,
        Err(e) => return fail(e.to_string()),
    };
    if said.is_nil() {
        return; // said nothing — correct and common
    }
    match crate::lua::word(said) {
        Ok(word) => town.talk(word),
        Err(e) => fail(format!("answered with a non-word: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::language::Read;
    use crate::testing::until;
    use crate::word::Tense;

    use crate::residents::inside::{end, word};
    use crate::residents::open;

    // what a resident watches or calls its own is said only by town: the square refuses it, and
    // still takes what anyone else may say. A reopen starts the set again from town's own
    // machinery.
    #[test]
    fn the_square_refuses_what_only_town_makes() {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        tokio::task::LocalSet::new().block_on(&rt, async {
            let dir = std::env::temp_dir().join(format!("town-inside-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            let file = dir.join("colors");
            std::fs::write(&file, "bg=#000\n").unwrap();
            let resident = dir.join("r.lua");
            let watch = format!("watch = {{ colors = {:?} }}", file.to_string_lossy());
            let lua = format!(
                r#"return {{ kinds = {{}}, listen = print, {watch}, own = {{ "noted" }} }}"#
            );
            std::fs::write(&resident, lua).unwrap();

            let town = Town::with_log(dir.join("past.log"));
            assert!(!town.admits(&word("work")), "town's own commands, from the start");
            assert!(!town.admits(&word("after")), "its own waits");
            assert!(!town.admits(&word("join")), "the programs it joins");
            assert!(!town.admits(&word("error")), "and its own failures");
            wire(&Lua::new(), &town, &resident, 0).unwrap();
            assert!(!town.admits(&word("colors")), "a watch speaks for its file");
            assert!(!town.admits(&word("noted")), "a resident's own");
            for k in ["hello", "note", "time"] {
                assert!(town.admits(&word(k)), "anyone may say {k}");
            }
            *town.inside.borrow_mut() = Town::machinery(); // as a reopen does
            assert!(town.admits(&word("noted")) && !town.admits(&word("work")));
            std::fs::remove_dir_all(&dir).ok();
        });
    }

    // a town is its programs, in the order given: a file that returns nothing is a library, and the
    // residents after it are written with what it defined
    #[test]
    fn a_library_comes_before_the_residents_that_use_it() {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        tokio::task::LocalSet::new().block_on(&rt, async {
            let dir = std::env::temp_dir().join(format!("town-programs-{}", std::process::id()));
            std::fs::create_dir_all(dir.join("residents")).unwrap();
            std::fs::write(dir.join("lib.lua"), "function greeting() return \"hello\" end")
                .unwrap();
            std::fs::write(
                dir.join("residents/echo.lua"),
                "return react { on(\"ping\", function() return event(\"pong\", greeting()) end) }",
            )
            .unwrap();

            let town = Town::with_log(dir.join("past.log"));
            let lua = Lua::new();
            install(&lua, &town).unwrap();
            open(&lua, &town, &[dir.join("lib.lua"), dir.join("residents")]);
            let mut words = town.words.subscribe();
            tokio::task::yield_now().await; // the listeners take their places
            town.talk(Word::new("ping", Tense::Past, Body::none()));
            let mut answer = None;
            for _ in 0..10 {
                tokio::task::yield_now().await;
                while let Ok(w) = words.try_recv() {
                    if w.kind == "pong" {
                        answer = Some(w)
                    }
                }
                if answer.is_some() {
                    break;
                }
            }
            assert_eq!(answer.expect("the resident answered").body, "hello");
            std::fs::remove_dir_all(&dir).ok();
        });
    }

    // a wait asked badly fails where it was asked: the asking resident's own error
    #[test]
    fn a_bad_wait_is_its_residents_error() {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        tokio::task::LocalSet::new().block_on(&rt, async {
            let dir = std::env::temp_dir().join(format!("town-wait-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            let town = Town::with_log(dir.join("past.log"));
            let lua = Lua::new();
            install(&lua, &town).unwrap();
            lua.load(crate::RESIDENT).exec().unwrap();
            let mut words = town.words.subscribe();
            for ms in ["-1", "'500'", "0/0", "math.huge"] {
                let asks = format!("function(w) return after({ms}, {{ about = 'x' }}) end");
                let asks = lua.load(asks).eval::<Function>().unwrap();
                listen(&lua, &asks, &town, "waiter", &word("ping"));
                let said = words.try_recv().expect("the bad wait is a word");
                assert_eq!(
                    (said.kind.as_str(), said.body.value()["from"].as_str()),
                    ("error", Some("waiter"))
                );
            }
            std::fs::remove_dir_all(&dir).ok();
        });
    }

    // a resident that fails is said as an `error` word: who, while hearing what, and why. One that
    // fails while hearing an `error` is only printed, so a broken listener cannot feed itself.
    #[test]
    fn a_failure_is_an_error_word() {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        tokio::task::LocalSet::new().block_on(&rt, async {
            let dir = std::env::temp_dir().join(format!("town-error-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            let town = Town::with_log(dir.join("past.log"));
            let lua = Lua::new();
            let broken =
                lua.load(r#"function(w) error("no " .. w.kind) end"#).eval::<Function>().unwrap();
            let mut words = town.words.subscribe();

            listen(&lua, &broken, &town, "broken", &word("ping"));
            let said = words.try_recv().expect("the failure is a word");
            assert_eq!(said.kind, "error");
            assert_eq!(said.tense, Tense::Past);
            assert_eq!(said.body.value()["from"], "broken");
            assert_eq!(said.body.value()["heard"], "ping");
            assert!(said.body.value()["message"].as_str().unwrap().contains("no ping"));

            listen(&lua, &broken, &town, "broken", &word("error"));
            assert!(words.try_recv().is_err(), "a failure on an error is not talked");
            std::fs::remove_dir_all(&dir).ok();
        });
    }

    // a resident that talks a non-word while hearing an `error` is only printed: it hears every
    // kind, so a talked failure would come back to it, and again, forever
    #[test]
    fn a_failed_talk_on_an_error_is_not_talked() {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        tokio::task::LocalSet::new().block_on(&rt, async {
            let dir = std::env::temp_dir().join(format!("town-talkloop-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            let town = Town::with_log(dir.join("past.log"));
            let lua = Lua::new();
            install(&lua, &town).unwrap();
            let bad = lua
                .load(r#"function(w) talk({ kind = "x", tense = "psat" }) end"#)
                .eval::<Function>()
                .unwrap();
            let mut words = town.words.subscribe();

            listen(&lua, &bad, &town, "bad", &word("ping"));
            let said = words.try_recv().expect("the failure is a word");
            assert_eq!(
                (
                    said.kind.as_str(),
                    said.body.value()["from"].as_str(),
                    said.body.value()["heard"].as_str()
                ),
                ("error", Some("bad"), Some("ping"))
            );

            listen(&lua, &bad, &town, "bad", &word("error"));
            assert!(words.try_recv().is_err(), "a failed talk on an error is not talked");
            std::fs::remove_dir_all(&dir).ok();
        });
    }

    // a program that fails to open is said once every resident listens, so one listening for
    // `error` hears it
    #[test]
    fn a_resident_hears_what_failed_to_open() {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        tokio::task::LocalSet::new().block_on(&rt, async {
            let dir = std::env::temp_dir().join(format!("town-openfail-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            let watcher =
                r#"return react { on("error", function(w) return event("saw", w.body.from) end) }"#;
            std::fs::write(dir.join("a-watcher.lua"), watcher).unwrap();
            std::fs::write(dir.join("b-broken.lua"), "return {").unwrap();

            let town = Town::with_log(dir.join("past.log"));
            let lua = Lua::new();
            install(&lua, &town).unwrap();
            let mut words = town.words.subscribe();
            open(&lua, &town, std::slice::from_ref(&dir));
            let mut saw = None;
            for _ in 0..10 {
                tokio::task::yield_now().await;
                while let Ok(w) = words.try_recv() {
                    if w.kind == "saw" {
                        saw = Some(w)
                    }
                }
                if saw.is_some() {
                    break;
                }
            }
            assert_eq!(saw.expect("the watcher heard the failure").body, "b-broken");
            std::fs::remove_dir_all(&dir).ok();
        });
    }

    // a fact said as the town opens is heard once: a watched file's first content, said before
    // the resident that listens for it first runs, comes in the greeting or as a word, not both
    #[test]
    fn a_fact_said_as_the_town_opens_is_heard_once() {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        tokio::task::LocalSet::new().block_on(&rt, async {
            let dir = std::env::temp_dir().join(format!("town-once-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            let file = dir.join("colors");
            std::fs::write(&file, "bg=#000").unwrap();
            let resident = dir.join("r.lua");
            let watch = format!("watch = {{ colors = {:?} }}", file.to_string_lossy());
            let lua = format!(
                r#"return {{ kinds = {{ "colors" }}, {watch},
                  listen = function(w) return event("heard", w.body) end }}"#
            );
            std::fs::write(&resident, lua).unwrap();
            let town = Town::with_log(dir.join("past.log"));
            let lua = Lua::new();
            install(&lua, &town).unwrap();
            let mut words = town.words.subscribe();
            open(&lua, &town, std::slice::from_ref(&resident));
            assert_eq!(until(&mut words, |w| w.kind == "heard").await.body, "bg=#000");
            // a save after: were the first heard twice, the next would be it again
            std::fs::write(&file, "bg=#111").unwrap();
            assert_eq!(until(&mut words, |w| w.kind == "heard").await.body, "bg=#111");
            end(&town);
            std::fs::remove_dir_all(&dir).ok();
        });
    }

    // a word on a line in `words`: its tense, if said, its kind, its body's key=value; written back
    // as it was read, and a word whose body it cannot hold is not written
    #[test]
    fn words_are_a_word_on_a_line() {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        tokio::task::LocalSet::new().block_on(&rt, async {
            let dir = std::env::temp_dir().join(format!("town-words-{}", std::process::id()));
            let town = Town::with_log(dir.join("past.log"));
            let lua = Lua::new();
            install(&lua, &town).unwrap();
            lua.load(crate::RESIDENT).exec().unwrap();
            let words = town.languages.get("words").unwrap();
            // a stream of words, the last of them not ended yet
            let said = &b"past\tshout\ttext=hi there\tn=\npastry\nshout\tno pair\nafter\tx=1"[..];
            let mut speech =
                crate::connection::Speech::new(crate::connection::Heard::new(said), words.clone());
            let mut errors = town.words.subscribe();
            let w = speech.next(&town, "me").await.unwrap();
            assert_eq!((w.kind.as_str(), w.tense), ("shout", Tense::Past));
            assert_eq!(w.body, Parts::from(serde_json::json!({ "text": "hi there", "n": "" })));
            let w = speech.next(&town, "me").await.unwrap();
            assert_eq!((w.kind.as_str(), w.tense), ("pastry", Tense::Present));
            assert!(speech.next(&town, "me").await.is_none(), "the rest is no word");
            let said: Vec<_> = std::iter::from_fn(|| errors.try_recv().ok())
                .map(|e| e.body.value()["said"].as_str().unwrap().to_string())
                .collect();
            assert_eq!(said, ["shout\tno pair", "after\tx=1"], "each line that is no word, once");
            let mut out = Vec::new();
            let said = Word::new("shout", Tense::Past, Body::of(Parts::map([("text", "hi")])));
            assert!(crate::connection::say(&mut out, &said, &words, &town.languages)
                .await
                .unwrap());
            assert!(crate::connection::say(&mut out, &word("bare"), &words, &town.languages)
                .await
                .unwrap());
            assert_eq!(out, b"past\tshout\ttext=hi\nfuture\tbare\n");
            let text = Word::new("note", Tense::Past, Body::of("a line"));
            assert!(
                !crate::connection::say(&mut out, &text, &words, &town.languages).await.unwrap(),
                "text is no pairs"
            );
            let deep = Parts::map([("names", Parts::List(vec!["a".into()]))]);
            let deep = Word::new("sessions", Tense::Present, Body::of(deep));
            assert!(
                !crate::connection::say(&mut out, &deep, &words, &town.languages).await.unwrap(),
                "a list is no value"
            );
        });
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

    /// the languages this repo has, which a town speaks once it names them
    pub(crate) const LANGUAGES: [&str; 3] = [
        include_str!("../languages/kv.lua"),
        include_str!("../languages/msgpack.lua"),
        include_str!("../languages/cbor.lua"),
    ];

    /// a town that speaks them
    fn town_speaking() -> Rc<Town> {
        let dir = std::env::temp_dir().join(format!("town-binary-{}", std::process::id()));
        let town = Town::with_log(dir.join("past.log"));
        let lua = Lua::new();
        install(&lua, &town).unwrap();
        lua.load(crate::RESIDENT).exec().unwrap();
        LANGUAGES.iter().for_each(|l| lua.load(*l).exec().unwrap());
        town
    }

    // msgpack, each value in its smallest form, and read back as it was
    #[test]
    fn msgpack_is_a_grammar() {
        use serde_json::json;
        let town = town_speaking();
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
        let ext = town.languages.parts("msgpack", b"\xd4\x00\x07").unwrap();
        assert_eq!(ext, Parts::map([("ext", Parts::from(0i64)), ("data", Parts::Bytes(vec![7]))]));
        assert!(town.languages.parts("msgpack", b"\x92\x01").is_err(), "half an array");
        assert_eq!(
            town.languages.get("msgpack").unwrap().read(b"\x92\x01", false, &town.languages),
            Read::More(3)
        );
    }

    // cbor, as RFC 8949's examples say it
    #[test]
    fn cbor_is_a_grammar() {
        use serde_json::json;
        let town = town_speaking();
        for (n, bytes) in [
            (json!(0), &b"\x00"[..]),
            (json!(23), b"\x17"),
            (json!(24), b"\x18\x18"),
            (json!(1000), b"\x19\x03\xe8"),
            (json!(1000000), b"\x1a\x00\x0f\x42\x40"),
            (json!(18446744073709551615u64), b"\x1b\xff\xff\xff\xff\xff\xff\xff\xff"),
            (json!(-1), b"\x20"),
            (json!(-10), b"\x29"),
            (json!(-100), b"\x38\x63"),
            (json!(-1000), b"\x39\x03\xe7"),
            (json!(1.5), b"\xf9\x3e\x00"),
            (json!(65504.5), b"\xfa\x47\x7f\xe0\x80"),
            (json!(1.1), b"\xfb\x3f\xf1\x99\x99\x99\x99\x99\x9a"),
            (json!(false), b"\xf4"),
            (json!(null), b"\xf6"),
            (json!("a"), b"\x61\x61"),
            (json!("\u{00fc}"), b"\x62\xc3\xbc"),
            (json!([1, [2, 3]]), b"\x82\x01\x82\x02\x03"),
            (json!({ "a": 1, "b": [2, 3] }), b"\xa2\x61a\x01\x61b\x82\x02\x03"),
        ] {
            both(&town, "cbor", n, bytes);
        }
        let raw = town.languages.parts("cbor", b"\x44\x01\x02\x03\x04").unwrap();
        assert_eq!(raw, Parts::Bytes(vec![1, 2, 3, 4]));
        assert_eq!(
            town.languages.parts("cbor", b"\xf9\x7c\x00").unwrap(),
            Parts::None,
            "infinity is no number"
        );
        let tagged = town.languages.parts("cbor", b"\xc1\x1a\x51\x4b\x67\xb0").unwrap();
        assert_eq!(tagged, Parts::from(json!({ "tag": 1, "value": 1363896240 })));
        let again = town.languages.bytes("cbor", &tagged).unwrap();
        assert_eq!(again, b"\xc1\x1a\x51\x4b\x67\xb0", "a tag is written as one");
    }

    // an ext of any size is read and written, its count the length of its data, and not kept
    #[test]
    fn msgpack_ext_is_every_size() {
        let town = town_speaking();
        let ext =
            |data: Vec<u8>| Parts::map([("ext", Parts::from(5i64)), ("data", Parts::Bytes(data))]);
        let mut eight = b"\xc7\x03\x05".to_vec();
        eight.extend([1, 2, 3]);
        assert_eq!(town.languages.bytes("msgpack", &ext(vec![1, 2, 3])).unwrap(), eight);
        assert_eq!(town.languages.parts("msgpack", &eight).unwrap(), ext(vec![1, 2, 3]));
        assert_eq!(
            town.languages.bytes("msgpack", &ext(vec![9])).unwrap(),
            b"\xd4\x05\x09",
            "the fixed size first"
        );
        let mut sixteen = b"\xc8\x01\x00\x05".to_vec();
        sixteen.extend(vec![7; 256]);
        assert_eq!(town.languages.parts("msgpack", &sixteen).unwrap(), ext(vec![7; 256]));
        assert_eq!(town.languages.bytes("msgpack", &ext(vec![7; 256])).unwrap(), sixteen);
    }

    // cbor of indefinite length, in pieces till a break, is read as the one value it is
    #[test]
    fn cbor_reads_what_comes_in_pieces() {
        use serde_json::json;
        let town = town_speaking();
        let read = |b: &[u8]| town.languages.parts("cbor", b).unwrap();
        assert_eq!(read(b"\x9f\x01\x82\x02\x03\xff"), Parts::from(json!([1, [2, 3]])));
        assert_eq!(
            read(b"\xbf\x61a\x01\x61b\x9f\x02\xff\xff"),
            Parts::from(json!({ "a": 1, "b": [2] }))
        );
        assert_eq!(read(b"\x7f\x65strea\x64ming\xff"), Parts::from("streaming"));
        assert_eq!(
            read(b"\x5f\x42\x01\x02\x43\x03\x04\x05\xff"),
            Parts::Bytes(vec![1, 2, 3, 4, 5])
        );
        assert_eq!(read(b"\x9f\xff"), Parts::List(vec![]));
        let nulls = Parts::List(vec![1i64.into(), Parts::None, 2i64.into()]);
        assert_eq!(read(b"\x9f\x01\xf6\x02\xff"), nulls, "a null keeps its place");
        assert_eq!(read(b"\xbf\xff"), Parts::Map(vec![]));
    }

    // a count may name a part read before it in the same seq; that part is the count, not kept,
    // and written as the length of what it counts
    #[test]
    fn a_count_may_name_a_part() {
        let town = town_speaking();
        let lua = Lua::new();
        install(&lua, &town).unwrap();
        lua.load(crate::RESIDENT).exec().unwrap();
        lua.load(r#"language("framed", seq{ { size = int{ size = 2 } }, text("size") })"#)
            .exec()
            .unwrap();
        assert_eq!(town.languages.parts("framed", b"\x00\x02hi").unwrap(), Parts::from("hi"));
        assert_eq!(town.languages.bytes("framed", &Parts::from("hey")).unwrap(), b"\x00\x03hey");
        let more =
            town.languages.get("framed").unwrap().read(b"\x00\x05hi", false, &town.languages);
        assert_eq!(more, Read::More(7), "the rest of what it counts is waited for");
    }

    // an empty table is parts by name, and `list{}` an empty list; a list that comes to Lua is
    // marked as one, and goes back as one
    #[test]
    fn an_empty_list_is_a_list() {
        let town = town_speaking();
        let lua = Lua::new();
        install(&lua, &town).unwrap();
        lua.load(crate::RESIDENT).exec().unwrap();
        let said = |code: &str| parts(lua.load(code).eval().unwrap()).unwrap();
        assert_eq!(said("return {}"), Parts::Map(vec![]));
        assert_eq!(said("return list{}"), Parts::List(vec![]));
        assert_eq!(said("return list{ 1, 2 }"), Parts::List(vec![1i64.into(), 2i64.into()]));
        let back = super::lua(&lua, &Parts::List(vec![])).unwrap();
        assert_eq!(parts(back).unwrap(), Parts::List(vec![]));
        let rpc = said(r#"return bytes("msgpack", { 0, 1, "nvim_get_mode", list{} })"#);
        assert_eq!(rpc, Parts::Bytes(b"\x94\x00\x01\xadnvim_get_mode\x90".to_vec()));
    }

    // a string named in a grammar is those bytes, kept as the part of that name
    #[test]
    fn a_named_string_is_kept() {
        let town = town_speaking();
        let lua = Lua::new();
        install(&lua, &town).unwrap();
        lua.load(crate::RESIDENT).exec().unwrap();
        lua.load(r#"language("mark", seq{ { is = "w" }, "~", { name = but("|") } })"#)
            .exec()
            .unwrap();
        let read = town.languages.parts("mark", b"w~alpha").unwrap();
        assert_eq!(read, Parts::from(serde_json::json!({ "is": "w", "name": "alpha" })));
        assert!(town.languages.parts("mark", b"p~alpha").is_err());
        // a part may be named for a piece: it is still a part
        lua.load(r#"language("note", seq{ { text = upto("\n") }, { map = but("\n") } })"#)
            .exec()
            .unwrap();
        let note = town.languages.parts("note", b"hi\nthere").unwrap();
        assert_eq!(note, Parts::from(serde_json::json!({ "text": "hi", "map": "there" })));
    }

    // in a binary language, bytes not of it are let go at once, so what comes after is heard; and a
    // header that says a unit is too big to hold is not of it
    #[test]
    fn a_binary_stream_goes_on() {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        rt.block_on(async {
            use tokio::io::AsyncWriteExt;
            let town = town_speaking();
            let msgpack = town.languages.get("msgpack").unwrap();
            let (mut to, from) = tokio::io::duplex(64);
            let mut heard = crate::connection::Heard::new(from);
            to.write_all(b"\xc1").await.unwrap();
            assert_eq!(heard.unit(&msgpack, &town.languages).await, Some(Err(vec![0xc1])));
            to.write_all(b"\x92\x01").await.unwrap();
            to.write_all(b"\xa1x").await.unwrap();
            let got = heard.unit(&msgpack, &town.languages).await.unwrap().unwrap();
            assert_eq!(got, Parts::List(vec![1i64.into(), "x".into()]));
            // stray bytes come with a word, in one read: they are let go, and the word is got
            to.write_all(b"\xc1\xc1\x92\x01\x02").await.unwrap();
            assert_eq!(heard.unit(&msgpack, &town.languages).await, Some(Err(vec![0xc1, 0xc1])));
            let got = heard.unit(&msgpack, &town.languages).await.unwrap().unwrap();
            assert_eq!(got, Parts::List(vec![1i64.into(), 2i64.into()]));
            to.write_all(b"\xc6\xff\xff\xff\xff").await.unwrap();
            assert!(
                heard.unit(&msgpack, &town.languages).await.unwrap().is_err(),
                "4 GiB is too much"
            );
            // a length at the end of what a number holds is too much too, not a little
            let cbor = town.languages.get("cbor").unwrap();
            to.write_all(b"\x5b\xff\xff\xff\xff\xff\xff\xff\xff").await.unwrap();
            assert!(heard.unit(&cbor, &town.languages).await.unwrap().is_err(), "no end to it");
        });
    }

    // a language that begins with itself ends: where it is already being read, it reads nothing
    #[test]
    fn a_language_may_name_itself() {
        let town = town_speaking();
        let lua = Lua::new();
        install(&lua, &town).unwrap();
        lua.load(crate::RESIDENT).exec().unwrap();
        let left = r#"language("x", choice{ seq{ ref("x"), "a" }, seq{ ref("x"), "b" }, "c" })"#;
        lua.load(left).exec().unwrap();
        assert!(town.languages.parts("x", b"z").is_err());
        assert_eq!(town.languages.parts("x", b"c"), Ok(Parts::from("c")));
        assert!(town.languages.bytes("x", &Parts::from("z")).is_err());
    }

    // a nothing in a list is null in Lua, and nothing again, so the list keeps its places
    #[test]
    fn lists_keep_their_places() {
        let lua = Lua::new();
        let list = Parts::List(vec![1i64.into(), Parts::None, "x".into()]);
        let held = super::lua(&lua, &list).unwrap();
        lua.globals().set("held", held.clone()).unwrap();
        assert_eq!(lua.load("return #held").eval::<i64>().unwrap(), 3);
        assert_eq!(parts(held).unwrap(), list);
    }

    // a resident that hears every kind hears the whole present as it opens
    #[test]
    fn every_kind_hears_the_whole_present() {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        tokio::task::LocalSet::new().block_on(&rt, async {
            let dir = std::env::temp_dir().join(format!("town-every-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            let resident = dir.join("r.lua");
            std::fs::write(
                &resident,
                r#"return react{ on({ tense = "present" }, function(w)
                return event("heard", w.kind) end) }"#,
            )
            .unwrap();
            let town = Town::with_log(dir.join("past.log"));
            town.talk(Word::new("theme", Tense::Present, Body::of("dark")));
            let lua = Lua::new();
            install(&lua, &town).unwrap();
            let mut words = town.words.subscribe();
            open(&lua, &town, std::slice::from_ref(&resident));
            let heard = async {
                for _ in 0..10 {
                    tokio::task::yield_now().await;
                    while let Ok(w) = words.try_recv() {
                        if w.kind == "heard" {
                            return Some(w.body);
                        }
                    }
                }
                None
            };
            assert_eq!(heard.await, Some(Body::of("theme")));
            std::fs::remove_dir_all(&dir).ok();
        });
    }

    // a clause with no kind matches every kind, so the resident hears every kind
    #[test]
    fn a_kindless_clause_hears_every_kind() {
        let lua = Lua::new();
        let dir = std::env::temp_dir().join(format!("town-kindless-{}", std::process::id()));
        install(&lua, &Town::with_log(dir.join("past.log"))).unwrap();
        lua.load(crate::RESIDENT).exec().unwrap();
        let kinds: Vec<String> = lua
            .load(r#"return react{ on("a", print), on({ tense = "past" }, print) }.kinds"#)
            .eval::<Vec<String>>()
            .unwrap();
        assert_eq!(kinds, ["*"]);
        let kinds: Vec<String> =
            lua.load(r#"return react{ on("error", print), on({}, print) }.kinds"#).eval().unwrap();
        assert_eq!(kinds, ["*", "error"], "every kind, and failings, named");
    }

    // a resident is its kinds, listen, watch and own, and nothing else: one with a field it does
    // not have, a `join` from before it was a word, is told so, and is not opened
    #[test]
    fn a_resident_is_only_what_it_can_be() {
        let dir = std::env::temp_dir().join(format!("town-fields-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let resident = dir.join("old.lua");
        std::fs::write(&resident, r#"return { kinds = { "x" }, join = { cat = { "cat" } } }"#)
            .unwrap();
        let town = Town::with_log(dir.join("past.log"));
        let e = wire(&Lua::new(), &town, &resident, 0).unwrap_err().to_string();
        assert!(e.contains("a resident has no `join`"), "{e}");
        std::fs::remove_dir_all(&dir).ok();
    }
}
