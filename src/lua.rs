//! The Lua residents. The README's *in Lua*.
//!
//! - **a resident**: a file that returns what it is, `{ kinds, listen, own, languages }`.
//! - **a library**: a file that returns nothing, for the files after it.
//! - **its window**: `present`, `past`, `talk` and the languages.
//! - **a word**: a table. A part is nil, a string, a number, a boolean, or a table: a list if its
//!   keys are 1 to n, else parts by name. A nothing in a list is `null`, so the list keeps its
//!   places.

use std::collections::BTreeMap;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::rc::{Rc, Weak};

use mlua::{Function, Lua, Table, Value};

use crate::parts::Parts;
use crate::residents::{named, who};
use crate::town::{Resident, Town};
use crate::voice::tell;
use crate::word::{hears, is_kind, Word};

/// Where the mark a list carries is kept.
const LIST: &str = "town.list";

/// parts, as Lua holds them
fn lua(lua: &Lua, p: &Parts) -> mlua::Result<Value> {
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
        // a Lua list holds no nil, so a nothing in it is `null`; and it is marked, so even an empty
        // one is a list again
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
fn parts(v: Value) -> mlua::Result<Parts> {
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

/// a table, as a word: its parts, as any resident's words are read, at the root
fn word(v: Value) -> mlua::Result<Word> {
    crate::voice::word_at("", &parts(v)?).map_err(mlua::Error::runtime)
}

/// What a file talks while it is read, held until it has opened, and said then, in order: one that
/// fails to open has done nothing. Nothing held, once it has opened.
type Held = Rc<std::cell::RefCell<Option<Vec<Word>>>>;

/// Where a resident's file runs, `_ENV`, all its own: its `talk`, how a resident is written, and
/// each library read before it, run here. So nothing is shared through Lua: one that comes again
/// begins again, and a library's functions speak as the resident they run for. Town's own windows,
/// `present`, `past`, the languages, are read through it. What it talks as it is read is `held`.
fn env(
    lua: &Lua,
    town: &Rc<Town>,
    who: &Resident,
    held: &Held,
    before: usize,
) -> mlua::Result<Table> {
    // a word said besides what it answers. One that is no word is the talker's error, raised
    // where it talks, so what hears it knows the kind in hand
    let (town, who, held) = (Rc::downgrade(town), who.clone(), held.clone());
    let talk = lua.create_function(move |_, w: Value| {
        let w = word(w).map_err(|e| mlua::Error::runtime(format!("talked a non-word: {e}")))?;
        match held.borrow_mut().as_mut() {
            Some(held) => held.push(w),
            None => still(&town)?.said(w, &who),
        }
        Ok(())
    })?;
    let env = lua.create_table_from([("talk", talk)])?;
    let all = lua.create_table_from([("__index", lua.globals())])?;
    env.set_metatable(Some(all));
    // its words, its own copy: how a resident is written, then the `before` libraries read before
    // it
    lua.load(crate::RESIDENT).set_name("resident").set_environment(env.clone()).exec()?;
    for library in libraries(lua)?.sequence_values::<Table>().take(before) {
        let library = library?;
        let (name, src): (String, String) = (library.get(1)?, library.get(2)?);
        lua.load(src).set_name(name).set_environment(env.clone()).exec()?;
    }
    Ok(env)
}

/// The town, as a resident's window onto it holds it: weakly, since the town keeps the Lua that
/// holds the window. Looked through once the town is gone, it is the looker's error: a nothing
/// would say there is no such fact, or var, when there is no town.
fn still(town: &Weak<Town>) -> mlua::Result<Rc<Town>> {
    town.upgrade().ok_or_else(|| mlua::Error::runtime("the town is gone"))
}

/// What a kind names, read: one kind, as `read` gives it; for `nvim/`, `/nvim` or `/`, each kind it
/// hears, by kind; for what names none, `a//b`, nothing.
fn node<T>(
    lua: &Lua,
    kind: &str,
    mut of: BTreeMap<String, T>,
    read: impl Fn(Option<T>) -> mlua::Result<Value>,
) -> mlua::Result<Value> {
    if is_kind(kind) {
        return read(of.remove(kind));
    }
    // what is under a kind, `nvim/`; it and what is under it, `/nvim`; every kind, `/`; else none
    let inner = crate::word::place(kind);
    if inner == kind || !(inner.is_empty() || is_kind(inner)) {
        return read(None);
    }
    let all = lua.create_table()?;
    for (k, v) in of {
        all.set(k, read(Some(v))?)?;
    }
    Ok(Value::Table(all))
}

/// How a resident is written, read again each time the town opens, after town's one rule, `hears`,
/// so what `on` keeps is the rule, not a `hears` a resident set before.
pub fn resident(lua: &Lua) -> mlua::Result<()> {
    let rule = lua.create_function(|_, (k, s): (String, String)| Ok(hears(&k, &s)))?;
    lua.globals().set("hears", rule)?;
    // the libraries read as the town opens, each given to the files after it
    lua.set_named_registry_value(LIBRARIES, lua.create_table()?)?;
    lua.set_named_registry_value(BEFORE, lua.create_table()?)?;
    lua.load(crate::RESIDENT).set_name("resident").exec()
}

/// Where the libraries a town has read are kept, in order: each a name and its source.
const LIBRARIES: &str = "town.libraries";

/// The libraries a town has read, in order: an empty list if it has read none.
fn libraries(lua: &Lua) -> mlua::Result<Table> {
    if let Some(libraries) = lua.named_registry_value::<Option<Table>>(LIBRARIES)? {
        return Ok(libraries);
    }
    lua.set_named_registry_value(LIBRARIES, lua.create_table()?)?;
    lua.named_registry_value(LIBRARIES)
}

/// How many libraries each file came after, the first time it came, by its path: so one that
/// comes again alone is given only those, as it was.
const BEFORE: &str = "town.before";

/// Give the residents their windows onto the town: `present`, `past`, and the languages. Each
/// has its own `talk` (`env`).
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

    // the world the town runs in, as a resident reads it: `os.getenv` reads its vars, not the
    // process's
    let world = Rc::downgrade(town);
    let getenv = lua.create_function(move |lua, var: mlua::String| {
        let var = var.as_bytes();
        still(&world)?
            .world
            .get(std::ffi::OsStr::from_bytes(&var))
            .map(|v| lua.create_string(v.as_bytes()))
            .transpose()
    })?;
    lua.globals().get::<Table>("os")?.set("getenv", getenv)?;

    // town itself, to name in a join: `join(kind, { TOWN, … })`
    let me = town.world.get(std::ffi::OsStr::new("TOWN")).map(|t| t.to_string_lossy().into_owned());
    lua.globals().set("TOWN", me)?;

    // a kind's fact now, and its recent facts, oldest first: for `nvim/`, `/nvim` or `/`, those of
    // each kind it hears, by kind
    let now = Rc::downgrade(town);
    lua.globals().set(
        "present",
        lua.create_function(move |lua, kind: String| {
            node(lua, &kind, still(&now)?.facts(&kind), |b| match b {
                Some(b) => self::lua(lua, b.parts()),
                None => Ok(Value::Nil),
            })
        })?,
    )?;
    let then = Rc::downgrade(town);
    lua.globals().set(
        "past",
        lua.create_function(move |lua, kind: String| {
            node(lua, &kind, still(&then)?.past(&kind), |bodies| {
                let bodies = bodies.unwrap_or_default().iter().map(|b| b.parts().clone()).collect();
                self::lua(lua, &Parts::List(bodies))
            })
        })?,
    )?;

    // languages: once said, any may read bytes as parts with one, or write parts as bytes. What
    // cannot be is the asker's error.
    let reads = Rc::downgrade(town);
    lua.globals().set(
        "parts",
        lua.create_function(move |lua, (name, bytes): (String, mlua::String)| {
            let parts = still(&reads)?.languages.parts(&name, &bytes.as_bytes());
            self::lua(lua, &parts.map_err(mlua::Error::runtime)?)
        })?,
    )?;
    let writes = Rc::downgrade(town);
    lua.globals().set(
        "bytes",
        lua.create_function(move |lua, (name, parts): (String, Value)| {
            let parts = self::parts(parts).map_err(|e| wrong(&name, e))?;
            let bytes = still(&writes)?.languages.bytes(&name, &parts);
            lua.create_string(bytes.map_err(mlua::Error::runtime)?)
        })?,
    )?;
    Ok(())
}

fn wrong(name: &str, e: impl std::fmt::Display) -> mlua::Error {
    mlua::Error::runtime(format!("{name}: {e}"))
}

/// Say what a file talked as it was read, now it has opened: as it, in order.
fn said(town: &Town, who: &Resident, held: &Held) {
    for w in held.take().unwrap_or_default() {
        town.said(w, who);
    }
}

/// A file, opened: what it talks as it is read is held until it has (`Held`). Failed, what it
/// talked is let go, and what it left talks as any does, from then. The number it is given, a
/// resident or a library.
pub(crate) fn wire(lua: &Lua, town: &Rc<Town>, path: &Path) -> mlua::Result<u64> {
    let held = Held::new(Some(Vec::new()).into());
    let wired = opened(lua, town, path, &held);
    held.take();
    wired
}

fn opened(lua: &Lua, town: &Rc<Town>, path: &Path, held: &Held) -> mlua::Result<u64> {
    let src = std::fs::read_to_string(path)?;
    // a resident in town's own process: what it says is its, as what it answers is; and it speaks
    // for none
    let who = Resident { lua: true, ..who(town, path) };
    // the libraries before it as it first came, whenever it comes again
    let place = path.to_string_lossy().into_owned();
    let before = match lua.named_registry_value::<Option<Table>>(BEFORE)? {
        Some(before) => before,
        None => {
            lua.set_named_registry_value(BEFORE, lua.create_table()?)?;
            lua.named_registry_value(BEFORE)?
        }
    };
    let n = match before.get::<Option<usize>>(place.as_str())? {
        Some(n) => n,
        None => {
            let n = libraries(lua)?.raw_len();
            before.set(place, n)?;
            n
        }
    };
    let env = env(lua, town, &who, held, n)?;
    let it: Option<Table> =
        lua.load(src.as_str()).set_name(named(path)).set_environment(env.clone()).eval()?;
    let Some(it) = it else {
        // a library: words for the files after it, each of which runs it in its own place
        libraries(lua)?.push(lua.create_sequence_from([named(path), src])?)?;
        said(town, &who, held);
        return Ok(who.id);
    };
    // nothing else: a misspelled field would do nothing, and say nothing of why
    for key in it.pairs::<Value, Value>().filter_map(Result::ok).map(|(k, _)| k) {
        let key = key.to_string().unwrap_or_default();
        if !["kinds", "listen", "own", "languages"].contains(&key.as_str()) {
            return Err(mlua::Error::runtime(format!(
                "a resident has no `{key}`: it is `kinds`, `listen`, `own` and `languages`"
            )));
        }
    }
    // what it is, read whole before it owns or hears anything, so one that cannot open holds none
    let strings = |t: Table| t.sequence_values::<String>().filter_map(Result::ok).collect();
    let own: Vec<String> = it.get::<Option<Table>>("own")?.map(strings).unwrap_or_default();
    let kinds: Option<Vec<String>> = it.get::<Option<Table>>("kinds")?.map(strings);
    let listens: Option<Function> = it.get("listen")?;
    let mut languages = BTreeMap::new();
    if let Some(spoken) = it.get::<Option<Table>>("languages")? {
        for pair in spoken.pairs::<String, Value>() {
            let (name, grammar) = pair?;
            languages.insert(name, parts(grammar)?);
        }
    }

    // own: kinds this resident alone says while it lives; before it hears, so one that may not own
    // what it names is not opened, and hears nothing
    let kinds = kinds.filter(|_| listens.is_some()).unwrap_or_default();
    let who = Resident { kinds, own, languages, ..who };
    let staying = town.come(&who).map_err(|e| wrong("what it is", e))?;
    // come, before it hears: as what it talked was said before it heard, so it hears none of it
    said(town, &who, held);

    // kinds: what wakes `listen`, and whatever it answers is talked. It hears from now, not from
    // when its task first runs, so what opening the town says after this, its failures among them,
    // is heard; and in town's own process it never stops reading, so no bound
    let hears = listens.map(|listens| (listens, town.words.hear(who.hearing(), None)));
    let (lua, here, id) = (lua.clone(), town.clone(), who.id);
    let task = tokio::task::spawn_local(async move {
        let _here = staying; // here for as long as it lives
        let Some((listens, words)) = hears else {
            return std::future::pending().await;
        };
        // told as any resident is, but `now`: within town it hears the present before any word,
        // so needs none to say what follows is now, and a clause of no kind would take it for one
        let to = async |w: &Word| {
            if !w.is_now() {
                listen(&lua, &listens, &here, &who, w);
            }
            Ok(true)
        };
        tell(&here, &who, words, to).await;
    });
    town.runs(id, task);
    Ok(id)
}

/// A resident listens to a word, a table { kind, tense, body, when }, and whatever it answers is
/// said by the one door, as its: a word, read as any resident's is. Nothing is nothing to say; a
/// Lua error, or an answer that is no word, is said as a `town/error` from it.
fn listen(lua: &Lua, listens: &Function, town: &Town, who: &Resident, w: &Word) {
    // a failure is its, hearing this kind: one hearing a `town/error` is printed, not said
    let fail = |message: String| town.error(who, Some(&w.kind), message);
    let said = self::lua(lua, &crate::voice::parts(w)).and_then(|t| listens.call::<Value>(t));
    match said.map(|said| (!said.is_nil()).then(|| word(said))) {
        Ok(None) => {}
        Ok(Some(Ok(word))) => town.said(word, who),
        Ok(Some(Err(e))) => fail(format!("answered with a non-word: {e}")),
        Err(e) => fail(e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::body::Body;
    use crate::language::Read;
    use crate::testing::{run, scratch, until, Scratch};
    use crate::voice::{bytes, say, Speech, Voice};
    use crate::word::Tense;

    use crate::residents::inside::{end, speaking, word};
    use crate::residents::open;

    // town's own words are not said from outside, in any tense; what a resident owns, it alone
    // says is or was, though anyone may want it; and a reopen lets go of what residents own
    #[test]
    fn what_is_held_is_said_by_who_holds_it() {
        run(async {
            let dir = scratch();
            let resident = dir.join("r.lua");
            let lua = r#"return { kinds = {}, listen = print, own = { "noted" } }"#;
            std::fs::write(&resident, lua).unwrap();
            let town = Town::with_log(dir.log());
            let (outside, inside) = (town.who("square", true), town.who("other", false));
            let may = |who: &Resident, kind: &str, tense: Tense| {
                town.may(&Word::new(kind, tense, Body::none()), who).is_ok()
            };
            let towns =
                ["work", "after", "join", "connect", "open", "error"].map(|w| format!("town/{w}"));
            for kind in &towns {
                assert!(!may(&outside, kind, Tense::Future) && !may(&outside, kind, Tense::Past));
                assert!(may(&inside, kind, Tense::Future), "what town runs may ask it: {kind}");
            }
            wire(&Lua::new(), &town, &resident).unwrap();
            assert!(!may(&outside, "noted", Tense::Past) && !may(&inside, "noted", Tense::Present));
            assert!(may(&outside, "noted", Tense::Future), "anyone may want it");
            for k in ["hello", "note", "time"] {
                assert!(may(&outside, k, Tense::Past), "anyone may say {k}");
            }
            town.reopen();
            assert!(
                may(&outside, "noted", Tense::Past) && !may(&outside, "town/work", Tense::Future)
            );
        });
    }

    // a language is a resident that is it: another that says it is one of the name is refused, and
    // the first is spoken still
    #[test]
    fn a_language_is_one_resident() {
        run(async {
            let dir = scratch();
            let x = |g: &str| format!("return {{ languages = {{ x = {g} }} }}");
            std::fs::write(dir.join("a.lua"), x(r#"upto("\n")"#)).unwrap();
            std::fs::write(dir.join("b.lua"), x(r#"but("\n")"#)).unwrap();
            let town = Town::with_log(dir.log());
            let lua = Lua::new();
            install(&lua, &town).unwrap();
            let mut words = town.words.listen();
            open(&lua, &town, &[dir.join("a.lua"), dir.join("b.lua")]);
            let refused = until(&mut words, |w| w.kind == "town/error").await;
            assert_eq!(refused.body.value()["from"], "b");
            assert!(refused.body.value()["message"].as_str().unwrap().contains("another is `x`"));
            assert_eq!(town.languages.parts("x", b"hi\n"), Ok(Parts::from("hi")), "a's, still");
            town.reopen();
        });
    }

    // a town is its residents, in the order given: a file that returns nothing is a library, and
    // the residents after it are written with what it defined
    #[test]
    fn a_library_comes_before_the_residents_that_use_it() {
        run(async {
            let dir = scratch();
            std::fs::create_dir_all(dir.join("residents")).unwrap();
            std::fs::write(dir.join("lib.lua"), "function greeting() return \"hello\" end")
                .unwrap();
            std::fs::write(
                dir.join("residents/echo.lua"),
                "return react { on(\"ping\", function() return event(\"pong\", greeting()) end) }",
            )
            .unwrap();

            let town = Town::with_log(dir.log());
            let lua = Lua::new();
            install(&lua, &town).unwrap();
            open(&lua, &town, &[dir.join("lib.lua"), dir.join("residents")]);
            let mut words = town.words.listen();
            town.talk(Word::new("ping", Tense::Past, Body::none()));
            assert_eq!(until(&mut words, |w| w.kind == "pong").await.body, "hello");
            town.reopen();
        });
    }

    // a wait asked badly fails where it was asked: the asking resident's own error
    #[test]
    fn a_bad_wait_is_its_residents_error() {
        let (_dir, town, lua) = written();
        let mut words = town.words.listen();
        for ms in ["-1", "'500'", "0/0", "math.huge"] {
            let asks = format!("function(w) return after({ms}, {{ about = 'x' }}) end");
            let asks = lua.load(asks).eval::<Function>().unwrap();
            listen(&lua, &asks, &town, &town.who("waiter", false), &word("ping"));
            let said = words.try_recv().expect("the bad wait is a word");
            assert_eq!(
                (said.kind.as_str(), said.body.value()["from"].as_str()),
                ("town/error", Some("waiter")),
                "after({ms})"
            );
        }
    }

    // a resident that fails, by a Lua error or a non-word talked or answered, is said as a
    // `town/error`: who, hearing what, and why. One that fails hearing a `town/error` is only
    // printed: two that hear errors would fail at each other's without end.
    #[test]
    fn a_failure_is_an_error_word() {
        let (_dir, town, lua) = written();
        let mut words = town.words.listen();
        for (fails, why) in [
            (r#"error("no " .. w.kind)"#, "no ping"),
            (r#"talk({ kind = "x", tense = "psat" })"#, "talked a non-word"),
            (r#"return { kind = "x", tense = "psat" }"#, "answered with a non-word"),
        ] {
            let broken = town.who("broken", false);
            let fails = lua.load(format!("function(w) {fails} end"));
            let fails =
                fails.set_environment(env(&lua, &town, &broken, &Held::default(), 0).unwrap());
            let fails = fails.eval::<Function>().unwrap();
            listen(&lua, &fails, &town, &broken, &word("ping"));
            let said = words.try_recv().expect("the failure is a word");
            assert_eq!((said.kind.as_str(), said.tense), ("town/error", Tense::Past), "{why}");
            assert_eq!(said.body.value()["from"], "broken", "{why}");
            assert_eq!(said.body.value()["heard"], "ping", "{why}");
            let message = said.body.value()["message"].as_str().unwrap().to_string();
            assert!(message.contains(why), "{message}");
            listen(&lua, &fails, &town, &broken, &word("town/error"));
            assert!(words.try_recv().is_err(), "a failure on an error is not talked: {why}");
        }
    }

    // a file that fails to open has done nothing: what it talked as it was read is not said, its
    // joins among them; one that opens says what it talked, in order, once it has come
    #[test]
    fn a_file_that_fails_to_open_does_nothing() {
        run(async {
            let dir = scratch();
            let talks =
                r#"talk(event("first")); talk(join("cat", { "cat" })); talk(event("second"))"#;
            std::fs::write(dir.join("a-fails.lua"), format!("{talks}; return {{ nope = 1 }}"))
                .unwrap();
            std::fs::write(
                dir.join("b-opens.lua"),
                format!("{talks}; return {{ own = {{ \"x\" }} }}"),
            )
            .unwrap();
            let town = Town::with_log(dir.log());
            let lua = Lua::new();
            install(&lua, &town).unwrap();
            let mut words = town.words.listen();
            open(&lua, &town, &[dir.join("a-fails.lua")]);
            let said = crate::testing::quiet(&mut words).await;
            let kinds: Vec<&str> = said.iter().map(|w| w.kind.as_str()).collect();
            assert_eq!(kinds, ["town/error"], "none of what it talked: {kinds:?}");
            assert!(town.joins.borrow().is_empty(), "nor joined");
            open(&lua, &town, &[dir.join("b-opens.lua")]);
            let said = crate::testing::quiet(&mut words).await;
            let at = |kind: &str| said.iter().position(|w| w.kind == kind).unwrap();
            let came = said.iter().position(|w| w.kind.ends_with("b-opens.lua")).unwrap();
            assert!(
                came < at("first")
                    && at("first") < at("town/join")
                    && at("town/join") < at("second")
            );
            town.reopen();
        });
    }

    // two that hear `town/error` and answer each with what is refused make no error of it, as
    // they would each other's without end: what is said while hearing one is refused quietly
    #[test]
    fn two_that_answer_errors_make_none() {
        run(async {
            let dir = scratch();
            let answers =
                r#"return react { on("town/error", function() return event("mine") end) }"#;
            std::fs::write(dir.join("a.lua"), answers).unwrap();
            std::fs::write(dir.join("b.lua"), answers).unwrap();
            std::fs::write(dir.join("c.lua"), r#"return { own = { "mine" } }"#).unwrap();
            let town = Town::with_log(dir.log());
            let lua = Lua::new();
            install(&lua, &town).unwrap();
            open(&lua, &town, &[dir.to_path_buf()]);
            let mut errors = town.words.hear(|_, w| w.kind == "town/error", None);
            town.error(&town.who("first", false), None, "one error".into());
            let mut said = 0;
            for _ in 0..64 {
                tokio::task::yield_now().await;
                said += std::iter::from_fn(|| errors.try_recv().ok()).count();
            }
            assert_eq!(said, 1, "the first, and none of what answers it");
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
            // the broken first, so the failure is heard only if it is said after all are wired
            std::fs::write(dir.join("a-broken.lua"), "return {").unwrap();
            std::fs::write(dir.join("b-watcher.lua"), watcher).unwrap();
            let town = Town::with_log(dir.log());
            let lua = Lua::new();
            install(&lua, &town).unwrap();
            let mut words = town.words.listen();
            open(&lua, &town, &[dir.to_path_buf()]);
            assert_eq!(until(&mut words, |w| w.kind == "saw").await.body, "a-broken");
            town.reopen();
        });
    }

    // a fact said as the town opens is heard once: said before the resident that listens for it
    // first runs, it comes in the greeting or as a word, not both
    #[test]
    fn a_fact_said_as_the_town_opens_is_heard_once() {
        run(async {
            let dir = scratch();
            let resident = dir.join("r.lua");
            let lua = r#"talk(fact("colors", "bg=#000"))
                return { kinds = { "colors" },
                  listen = function(w) return event("heard", w.body) end }"#;
            std::fs::write(&resident, lua).unwrap();
            let town = Town::with_log(dir.log());
            let lua = Lua::new();
            install(&lua, &town).unwrap();
            let mut words = town.words.listen();
            open(&lua, &town, std::slice::from_ref(&resident));
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
            let caller = r#"return react { on("ping", function()
                later(); later(); mine("left")
              end) }"#;
            std::fs::write(dir.join("a-owner.lua"), owner).unwrap();
            std::fs::write(dir.join("b-caller.lua"), caller).unwrap();
            let town = Town::with_log(dir.log());
            let lua = Lua::new();
            install(&lua, &town).unwrap();
            let mut words = town.words.listen();
            open(&lua, &town, &[dir.join("a-owner.lua"), dir.join("b-caller.lua")]);
            let place = crate::residents::whole(&dir.join("a-owner.lua"));
            let here =
                format!("town/residents/{}", place.to_string_lossy().trim_start_matches('/'));
            let id = town.facts(&here)[&here].value()["id"].as_u64().unwrap();
            assert_eq!(until(&mut words, |w| w.kind == "opened").await.by, id, "as it opens");
            town.talk(Word::new("ping", Tense::Past, Body::none()));
            for said in ["once", "again", "left"] {
                let w = until(&mut words, |w| w.kind == "mine" || w.kind == "town/error").await;
                assert_eq!((w.kind.as_str(), w.by), ("mine", id), "{said}: {:?}", w.body);
                assert_eq!(w.body, said);
            }
            town.reopen();
        });
    }

    // a library is words, not a place to keep things: each resident runs it in its own place, so
    // what it defines, and what a library keeps, is that resident's own, unseen by the others
    #[test]
    fn a_residents_names_and_its_librarys_are_its_own() {
        run(async {
            let dir = scratch();
            let lib = "count = 0; function bump() count = count + 1; return count end";
            let a = r#"secret = "a's"; talk(event("bumped", bump())); return {}"#;
            let b = r#"talk(event("seen", tostring(secret))); talk(event("count", count))
                talk(event("bumped", bump())); return {}"#;
            let files = [("a-lib.lua", lib), ("b-a.lua", a), ("c-b.lua", b)];
            for (name, src) in files {
                std::fs::write(dir.join(name), src).unwrap();
            }
            let town = Town::with_log(dir.log());
            let lua = Lua::new();
            install(&lua, &town).unwrap();
            let mut words = town.words.listen();
            open(&lua, &town, &files.map(|(name, _)| dir.join(name)));
            let said = |kind: &'static str| move |w: &Word| w.kind == kind;
            assert_eq!(until(&mut words, said("bumped")).await.body, Body::of(1i64));
            assert_eq!(until(&mut words, said("seen")).await.body, "nil", "a's own, unseen by b");
            let count = until(&mut words, said("count")).await.body;
            assert_eq!(count, Body::of(0i64), "b's own copy of the library");
            assert_eq!(
                until(&mut words, said("bumped")).await.body,
                Body::of(1i64),
                "its own count"
            );
            town.reopen();
        });
    }

    // a word on a line in `words`: its tense, if said, its kind, its body's key=value; written back
    // as it was read, and a word whose body it cannot hold is not written
    #[test]
    fn words_are_a_word_on_a_line() {
        run(async {
            let (_dir, town, _lua) = town_speaking();
            let words = town.languages.get("words").unwrap();
            // a stream of words, the last of them not ended yet
            let said = &b"past\tshout\ttext=hi there\tn=\npastry\nshout\tno pair\nafter\tx=1"[..];
            let mut speech = Speech::new(Voice::new(said), "words");
            let mut errors = town.words.listen();
            let me = Resident::town();
            let w = speech.next(&town, &me).await.unwrap();
            assert_eq!((w.kind.as_str(), w.tense), ("shout", Tense::Past));
            assert_eq!(w.body, Parts::from(serde_json::json!({ "text": "hi there", "n": "" })));
            let w = speech.next(&town, &me).await.unwrap();
            assert_eq!((w.kind.as_str(), w.tense), ("pastry", Tense::Present));
            assert!(speech.next(&town, &me).await.is_none(), "the rest is no word");
            let said: Vec<_> = std::iter::from_fn(|| errors.try_recv().ok())
                .map(|e| e.body.value()["said"].as_str().unwrap().to_string())
                .collect();
            assert_eq!(said, ["shout\tno pair", "after\tx=1"], "each line that is no word, once");
            let mut out = Vec::new();
            let said = Word::new("shout", Tense::Past, Body::of(Parts::map([("text", "hi")])));
            assert!(say(&mut out, &said, &words, &town.languages).await.unwrap());
            assert!(say(&mut out, &word("bare"), &words, &town.languages).await.unwrap());
            assert_eq!(out, b"past\tshout\ttext=hi\nfuture\tbare\n");
            let text = Word::new("note", Tense::Past, Body::of("a line"));
            assert!(
                !say(&mut out, &text, &words, &town.languages).await.unwrap(),
                "text is no pairs"
            );
            let deep = Parts::map([("names", Parts::List(vec!["a".into()]))]);
            let deep = Word::new("sessions", Tense::Present, Body::of(deep));
            assert!(
                !say(&mut out, &deep, &words, &town.languages).await.unwrap(),
                "a list is no value"
            );
            // a word of no body reads as none, and is written as it was read
            let bare = &b"present\tnvim\n"[..];
            let read = town.languages.parts("words", bare).unwrap();
            assert_eq!(read.get("body").cloned().unwrap_or_default(), Parts::None, "{read:?}");
            let none = Word::new("nvim", Tense::Present, Body::none());
            assert_eq!(bytes(&none, &words, &town.languages).unwrap(), bare);
            // and that what follows is now, a past word at "", which reads back as it
            let now = bytes(&Word::now(), &words, &town.languages).unwrap();
            assert_eq!(now, b"past\t\n");
            let read = town.languages.parts("words", &now).unwrap();
            assert!(crate::voice::of(&read).unwrap().is_now(), "{read:?}");
            assert!(town.languages.parts("words", b"\n").is_err(), "a blank line is no word");
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

    /// a town of a test's own, and a Lua that writes its residents
    fn written() -> (Scratch, Rc<Town>, Lua) {
        let dir = scratch();
        let town = Town::with_log(dir.log());
        let lua = Lua::new();
        install(&lua, &town).unwrap();
        resident(&lua).unwrap();
        (dir, town, lua)
    }

    /// such a town, opened on town's own languages, so they are residents here, as in any town
    fn town_speaking() -> (Scratch, Rc<Town>, Lua) {
        let (dir, town, lua) = written();
        open(&lua, &town, &[crate::residents::own().join("languages")]);
        (dir, town, lua)
    }

    // msgpack, each value in its smallest form, and read back as it was
    #[test]
    fn msgpack_is_a_grammar() {
        run(async {
            use serde_json::json;
            let (_dir, town, _lua) = town_speaking();
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
            assert_eq!(
                town.languages.get("msgpack").unwrap().read(b"\x92\x01", false, &town.languages),
                Read::More(3)
            );
        });
    }

    // cbor, as RFC 8949's examples say it
    #[test]
    fn cbor_is_a_grammar() {
        run(async {
            use serde_json::json;
            let (_dir, town, _lua) = town_speaking();
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
        });
    }

    // an ext of any size is read, and written in the smallest form that holds it; its count is the
    // length of its data, and not kept
    #[test]
    fn msgpack_ext_is_every_size() {
        run(async {
            let (_dir, town, _lua) = town_speaking();
            let mut sixteen = b"\xc8\x01\x00\x05".to_vec();
            sixteen.extend([7; 256]);
            for (data, bytes) in [
                (vec![9], b"\xd4\x05\x09".to_vec()),
                (vec![1, 2, 3], b"\xc7\x03\x05\x01\x02\x03".to_vec()),
                (vec![7; 256], sixteen),
            ] {
                let ext = Parts::map([("ext", Parts::from(5i64)), ("data", Parts::Bytes(data))]);
                assert_eq!(town.languages.bytes("msgpack", &ext).unwrap(), bytes, "{ext:?}");
                assert_eq!(town.languages.parts("msgpack", &bytes).unwrap(), ext, "{bytes:02x?}");
            }
        });
    }

    // cbor of indefinite length, in pieces till a break, is read as the one value it is
    #[test]
    fn cbor_reads_what_comes_in_pieces() {
        run(async {
            use serde_json::json;
            let (_dir, town, _lua) = town_speaking();
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
        });
    }

    // a count may name a part read before it in the same seq; that part is the count, not kept,
    // and written as the length of what it counts
    #[test]
    fn a_count_may_name_a_part() {
        run(async {
            let (dir, town, _lua) = written();
            speaking(&town, &dir, "framed", r#"seq{ { size = int{ size = 2 } }, text("size") }"#);
            assert_eq!(town.languages.parts("framed", b"\x00\x02hi").unwrap(), Parts::from("hi"));
            assert_eq!(
                town.languages.bytes("framed", &Parts::from("hey")).unwrap(),
                b"\x00\x03hey"
            );
            let more =
                town.languages.get("framed").unwrap().read(b"\x00\x05hi", false, &town.languages);
            assert_eq!(more, Read::More(7), "the rest of what it counts is waited for");
        });
    }

    // an empty table is parts by name, and `list{}` an empty list; a list that comes to Lua is
    // marked as one, and goes back as one
    #[test]
    fn an_empty_list_is_a_list() {
        run(async {
            let (_dir, _town, lua) = town_speaking();
            let said = |code: &str| parts(lua.load(code).eval().unwrap()).unwrap();
            assert_eq!(said("return {}"), Parts::Map(vec![]));
            assert_eq!(said("return list{}"), Parts::List(vec![]));
            assert_eq!(said("return list{ 1, 2 }"), Parts::List(vec![1i64.into(), 2i64.into()]));
            assert_eq!(said(r#"return past("none")"#), Parts::List(vec![]), "no facts, a list");
            let back = super::lua(&lua, &Parts::List(vec![])).unwrap();
            assert_eq!(parts(back).unwrap(), Parts::List(vec![]));
            let rpc = said(r#"return bytes("msgpack", { 0, 1, "nvim_get_mode", list{} })"#);
            assert_eq!(rpc, Parts::Bytes(b"\x94\x00\x01\xadnvim_get_mode\x90".to_vec()));
        });
    }

    // a string named in a grammar is those bytes, kept as the part of that name
    #[test]
    fn a_named_string_is_kept() {
        run(async {
            let (dir, town, _lua) = written();
            speaking(&town, &dir, "mark", r#"seq{ { is = "w" }, "~", { name = but("|") } }"#);
            let read = town.languages.parts("mark", b"w~alpha").unwrap();
            assert_eq!(read, Parts::from(serde_json::json!({ "is": "w", "name": "alpha" })));
            assert!(town.languages.parts("mark", b"p~alpha").is_err());
            // a part may be named for a piece: it is still a part
            speaking(&town, &dir, "note", r#"seq{ { text = upto("\n") }, { map = but("\n") } }"#);
            let note = town.languages.parts("note", b"hi\nthere").unwrap();
            assert_eq!(note, Parts::from(serde_json::json!({ "text": "hi", "map": "there" })));
        });
    }

    // in a binary language, bytes not of it are let go at once, so what comes after is heard; and a
    // header that says a unit is too big to hold is not of it
    #[test]
    fn a_binary_stream_goes_on() {
        run(async {
            use tokio::io::AsyncWriteExt;
            let (_dir, town, _lua) = town_speaking();
            let (mut to, from) = tokio::io::duplex(64);
            let mut voice = Voice::new(from);
            to.write_all(b"\xc1").await.unwrap();
            assert_eq!(voice.unit("msgpack", &town.languages).await, Some(Err(vec![0xc1])));
            to.write_all(b"\x92\x01").await.unwrap();
            to.write_all(b"\xa1x").await.unwrap();
            let got = voice.unit("msgpack", &town.languages).await.unwrap().unwrap();
            assert_eq!(got, Parts::List(vec![1i64.into(), "x".into()]));
            // stray bytes come with a word, in one read: they are let go, and the word is got
            to.write_all(b"\xc1\xc1\x92\x01\x02").await.unwrap();
            assert_eq!(voice.unit("msgpack", &town.languages).await, Some(Err(vec![0xc1, 0xc1])));
            let got = voice.unit("msgpack", &town.languages).await.unwrap().unwrap();
            assert_eq!(got, Parts::List(vec![1i64.into(), 2i64.into()]));
            to.write_all(b"\xc6\xff\xff\xff\xff").await.unwrap();
            assert!(
                voice.unit("msgpack", &town.languages).await.unwrap().is_err(),
                "4 GiB is too much"
            );
            // a length at the end of what a number holds is too much too, not a little
            to.write_all(b"\x5b\xff\xff\xff\xff\xff\xff\xff\xff").await.unwrap();
            assert!(voice.unit("cbor", &town.languages).await.unwrap().is_err(), "no end to it");
        });
    }

    // a language that begins with itself ends: where it is already being read, it reads nothing
    #[test]
    fn a_language_may_name_itself() {
        run(async {
            let (dir, town, _lua) = written();
            speaking(
                &town,
                &dir,
                "x",
                r#"choice{ seq{ ref("x"), "a" }, seq{ ref("x"), "b" }, "c" }"#,
            );
            assert!(town.languages.parts("x", b"z").is_err());
            assert_eq!(town.languages.parts("x", b"c"), Ok(Parts::from("c")));
            assert!(town.languages.bytes("x", &Parts::from("z")).is_err());
        });
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
        run(async {
            let dir = scratch();
            let resident = dir.join("r.lua");
            std::fs::write(
                &resident,
                r#"return react{ on({ tense = "present" }, function(w)
                return event("heard", w.kind) end) }"#,
            )
            .unwrap();
            let town = Town::with_log(dir.log());
            town.talk(Word::new("theme", Tense::Present, Body::of("dark")));
            let lua = Lua::new();
            install(&lua, &town).unwrap();
            let mut words = town.words.listen();
            open(&lua, &town, std::slice::from_ref(&resident));
            assert_eq!(until(&mut words, |w| w.kind == "heard").await.body, "theme");
            town.reopen();
        });
    }

    // a resident of clauses hears the kinds they name, and every kind if one names none; errors
    // only if a clause names them. A word wakes the first clause that hears its kind, or none.
    #[test]
    fn a_resident_hears_what_its_clauses_name() {
        let (_dir, _town, lua) = written();
        let kinds = |clauses: &str| -> Vec<String> {
            lua.load(format!("return react{{ {clauses} }}.kinds")).eval().unwrap()
        };
        assert_eq!(kinds(r#"on("/peer", print), on("note", print)"#), ["/peer", "note"]);
        assert_eq!(kinds(r#"on("a", print), on({ tense = "past" }, print)"#), ["/"], "but errors");
        assert_eq!(kinds(r#"on("town/error", print), on({}, print)"#), ["/", "town/error"]);
        let r: Table = lua
            .load(
                r#"return react{ on("/peer", function(w) return w.kind end), on("note", print) }"#,
            )
            .eval()
            .unwrap();
        let listen: Function = r.get("listen").unwrap();
        let heard = |kind: &str| -> Option<String> {
            let w: Table = lua.load(format!("return {{ kind = {kind:?} }}")).eval().unwrap();
            listen.call(w).unwrap()
        };
        assert_eq!((heard("peer"), heard("peer/a")), (Some("peer".into()), Some("peer/a".into())));
        assert_eq!((heard("peers"), heard("note/a")), (None, None), "beside it, or under `note`");
    }

    // kinds that nest hear a word once: `/nvim` and `nvim/1` both hear `nvim/1`
    #[test]
    fn nested_kinds_hear_a_word_once() {
        run(async {
            let dir = scratch();
            let resident = dir.join("r.lua");
            std::fs::write(
                &resident,
                r#"return { kinds = { "/nvim", "nvim/1" }, listen = function(w)
                     return { kind = "heard", tense = "past", body = w.kind } end }"#,
            )
            .unwrap();
            let town = Town::with_log(dir.log());
            let mut words = town.words.listen();
            wire(&Lua::new(), &town, &resident).unwrap();
            for kind in ["nvim/1", "nvim/2"] {
                town.talk(Word::new(kind, Tense::Past, Body::none()));
            }
            let mut next = async || until(&mut words, |w| w.kind == "heard").await.body;
            assert_eq!((next().await, next().await), (Body::of("nvim/1"), Body::of("nvim/2")));
            town.reopen();
        });
    }

    // a resident's own `hears` is its own: the town opened again, `on` keeps town's rule
    #[test]
    fn a_residents_own_hears_is_not_the_rule() {
        let (_dir, _town, lua) = written();
        lua.load("function hears() return true end").exec().unwrap();
        resident(&lua).unwrap(); // as the town opens again
        let matched: bool =
            lua.load(r#"return on("a", print).match({ kind = "b" })"#).eval().unwrap();
        assert!(!matched);
    }

    // `present` and `past` read a kind alike: its own, or for `kind/`, each kind under it, by kind;
    // never one beside it, and nothing for what names none
    #[test]
    fn a_kind_is_read_as_a_node_or_a_folder() {
        let lua = Lua::new();
        let dir = scratch();
        let town = Town::with_log(dir.log());
        install(&lua, &town).unwrap();
        for (kind, n) in
            [("nvims", 1i64), ("nvims/1", 1), ("nvims/1", 2), ("nvims/2/a", 1), ("nvimsx", 1)]
        {
            town.talk(Word::new(kind, Tense::Present, Body::of(n)));
        }
        // a folder, as `kind=each(what it holds)`, sorted
        lua.load(
            r#"function folder(t, each) local out = {}
                 for k, v in pairs(t) do out[#out + 1] = k .. "=" .. each(v) end
                 table.sort(out); return table.concat(out, " ") end"#,
        )
        .exec()
        .unwrap();
        let read = |code: &str| -> String { lua.load(code).eval().unwrap() };
        assert_eq!(read(r#"return present("nvims")"#), "1");
        assert_eq!(read(r#"return folder(present("nvims/"), tostring)"#), "nvims/1=2 nvims/2/a=1");
        assert_eq!(read(r#"return table.concat(past("nvims/1"), " ")"#), "1 2");
        assert_eq!(
            read(r#"return folder(past("nvims/"), function(v) return table.concat(v, ",") end)"#),
            "nvims/1=1,2 nvims/2/a=1"
        );
        assert_eq!(read(r#"return #past("nvims/1/") .. #past("none")"#), "00");
        assert_eq!(read(r#"return tostring(present("")) .. tostring(present("a//b"))"#), "nilnil");
    }

    // a resident connects to as many as it likes, each a kind under one, and one clause hears them
    // all, each by its kind
    #[test]
    fn a_resident_hears_each_join_under_its_kind() {
        run(async {
            let dir = scratch();
            let town = Town::with_log(dir.log());
            // two other ends, each saying who it is
            for name in ["a", "b"] {
                let listener = tokio::net::UnixListener::bind(dir.join(name)).unwrap();
                tokio::task::spawn_local(async move {
                    use tokio::io::AsyncWriteExt;
                    let (mut conn, _) = listener.accept().await.unwrap();
                    conn.write_all(format!("from {name}\n").as_bytes()).await.unwrap();
                    std::future::pending::<()>().await;
                });
            }
            let at = format!("{}/", dir.display());
            std::fs::write(
                dir.join("peers.lua"),
                format!(
                    r#"local said = seq{{ {{ tense = be("", "past") }},
                         {{ body = seq{{ {{ said = upto("\n") }} }} }} }}
                       for _, name in ipairs({{ "a", "b" }}) do
                         talk(connect("peer/" .. name, {:?} .. name, {{ speaks = "said" }}))
                       end
                       return react {{ languages = {{ said = said }},
                         on({{ kind = "peer/", tense = "past", by = "said" }}, function(w)
                           return event("heard", {{ from = w.kind, said = w.body.said }})
                         end) }}"#,
                    at
                ),
            )
            .unwrap();
            let lua = Lua::new();
            install(&lua, &town).unwrap();
            let mut words = town.words.listen();
            open(&lua, &town, &[dir.join("peers.lua")]);
            let mut heard = Vec::new();
            for _ in 0..2 {
                let w = until(&mut words, |w| w.kind == "heard").await;
                heard.push((w.body.value()["from"].clone(), w.body.value()["said"].clone()));
            }
            heard.sort_by_key(|h| h.0.to_string());
            assert_eq!(
                heard,
                [
                    (serde_json::json!("peer/a"), serde_json::json!("from a")),
                    (serde_json::json!("peer/b"), serde_json::json!("from b"))
                ]
            );
        });
    }

    // a resident is its kinds, listen and own, and nothing else: one with a field it does not have,
    // such as `watch`, is told so, and is not opened
    #[test]
    fn a_resident_is_only_what_it_can_be() {
        let dir = scratch();
        let resident = dir.join("old.lua");
        std::fs::write(&resident, r#"return { kinds = { "x" }, watch = { colors = "colors" } }"#)
            .unwrap();
        let town = Town::with_log(dir.log());
        let e = wire(&Lua::new(), &town, &resident).unwrap_err().to_string();
        assert!(e.contains("a resident has no `watch`"), "{e}");
    }

    // a resident that cannot open owns nothing: what it is, it says whole before it owns
    #[test]
    fn a_resident_that_cannot_open_owns_nothing() {
        let dir = scratch();
        let resident = dir.join("r.lua");
        std::fs::write(
            &resident,
            r#"return { kinds = { "x" }, listen = "oops", own = { "noted" } }"#,
        )
        .unwrap();
        let town = Town::with_log(dir.log());
        assert!(wire(&Lua::new(), &town, &resident).is_err());
        let noted = Word::new("noted", Tense::Past, Body::none());
        assert!(town.may(&noted, &town.who("square", true)).is_ok(), "none owns `noted`");
    }

    // a resident owns its kinds for as long as it lives: ended, however it ends, it lets go
    #[test]
    fn a_resident_owns_while_it_lives() {
        run(async {
            let dir = scratch();
            let town = Town::with_log(dir.log());
            let noted = Word::new("noted", Tense::Past, Body::none());
            let square = town.who("square", true);
            for lua in [
                r#"return { kinds = {}, listen = print, own = { "noted" } }"#,
                r#"return { own = { "noted" } }"#,
            ] {
                std::fs::write(dir.join("r.lua"), lua).unwrap();
                wire(&Lua::new(), &town, &dir.join("r.lua")).unwrap();
                assert!(town.may(&noted, &square).is_err(), "it owns `noted`: {lua}");
                end(&town);
                for _ in 0..3 {
                    tokio::task::yield_now().await;
                }
                assert!(town.may(&noted, &square).is_ok(), "ended, it lets go: {lua}");
            }
        });
    }

    // a file that cannot be posted is its resident's error, not lost without a word
    #[test]
    fn a_failed_post_is_an_error() {
        let (dir, _town, lua) = written();
        std::fs::create_dir_all(dir.join("folder")).unwrap();
        let post =
            |path: &Path| lua.load(format!("post({:?}, 'x')", path.to_str().unwrap())).exec();
        assert!(post(Path::new("/nonexistent/dir/f")).is_err(), "no folder to write in");
        assert!(post(&dir.join("folder")).is_err(), "a folder is no file");
        assert!(!dir.join("folder.writing").exists(), "what was written aside is gone");
        post(&dir.join("f")).unwrap();
        assert_eq!(std::fs::read_to_string(dir.join("f")).unwrap(), "x");
    }
}
