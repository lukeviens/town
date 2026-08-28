//! The residents: townsfolk who listen to words and talk words back. Each returns a
//! { listen, talk } — the kinds it listens for, and a function whose return it talks.
//! A resident may also `watch` a file (talked in as a present word on change) or a
//! directory (a present word on any change within). `present(kind)`/`past(kind)` read
//! the town; `reopen()` re-reads every resident from disk, so town can hot-reload itself.

use std::rc::Rc;

use mlua::{Function, Lua, LuaSerdeExt, Table, Value as LuaValue};
use notify::Watcher;
use serde_json::Value;

use crate::paths;
use crate::town::Town;
use crate::word::{Tense, Word};

/// Give the residents their windows onto the town: `present`, `past`, and `reopen`.
pub fn install(lua: &Lua, town: &Rc<Town>) -> mlua::Result<()> {
    let now = town.clone();
    lua.globals().set(
        "present",
        lua.create_function(move |lua, kind: String| match now.present.borrow().get(&kind) {
            Some(v) => lua.to_value(v),
            None => Ok(LuaValue::Nil),
        })?,
    )?;

    let then = town.clone();
    lua.globals().set(
        "past",
        lua.create_function(move |lua, kind: String| lua.to_value(&then.past(&kind)))?,
    )?;

    let re_lua = lua.clone();
    let re_town = town.clone();
    lua.globals().set(
        "reopen",
        lua.create_function(move |_, ()| {
            if let Err(e) = open(&re_lua, &re_town) {
                eprintln!("town: reopen: {e}");
            }
            Ok(())
        })?,
    )?;
    Ok(())
}

/// Re-read every residents/*.lua and bring it to life. Bumps the town's age so the
/// previous generation of listeners retires; called once at boot and on every reload.
pub fn open(lua: &Lua, town: &Rc<Town>) -> mlua::Result<()> {
    town.age.set(town.age.get() + 1);
    let age = town.age.get();

    // the composition vocabulary first, so residents can be written with it
    if let Ok(src) = std::fs::read_to_string(paths::lib()) {
        if let Err(e) = lua.load(src).set_name("lib").exec() {
            eprintln!("town: lib: {e}");
        }
    }

    let mut names: Vec<String> = std::fs::read_dir(paths::residents())
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let p = e.path();
            (p.extension().and_then(|s| s.to_str()) == Some("lua"))
                .then(|| p.file_stem().and_then(|s| s.to_str()).map(String::from))
                .flatten()
        })
        .collect();
    names.sort();
    for name in names {
        if let Err(e) = wire(lua, town, &name, age) {
            eprintln!("town: {name}: {e}");
        }
    }
    Ok(())
}

fn wire(lua: &Lua, town: &Rc<Town>, name: &str, age: u64) -> mlua::Result<()> {
    let src = std::fs::read_to_string(paths::residents().join(format!("{name}.lua")))?;
    let spec: Table = lua.load(src).set_name(name).eval()?;

    // watch: a file talked in as its content, a directory as a bare change signal —
    // driven by real filesystem events (FSEvents/inotify), never a tick.
    if let Some(watch) = spec.get::<Option<Table>>("watch")? {
        for pair in watch.pairs::<String, String>() {
            let (kind, path) = pair?;
            let town = town.clone();
            let dir = std::fs::metadata(&path).map(|m| m.is_dir()).unwrap_or(false);
            tokio::task::spawn_local(async move {
                let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
                let mut watcher = match notify::recommended_watcher(move |_| { let _ = tx.send(()); }) {
                    Ok(w) => w,
                    Err(_) => return,
                };
                if watcher
                    .watch(std::path::Path::new(&path), notify::RecursiveMode::NonRecursive)
                    .is_err()
                {
                    return;
                }
                // a file greets with its current content (so theme is set on boot); a directory doesn't
                if !dir {
                    if let Ok(text) = std::fs::read_to_string(&path) {
                        town.talk(Word { kind: kind.clone(), tense: Tense::Present, body: Value::String(text) });
                    }
                }
                while rx.recv().await.is_some() {
                    if age < town.age.get() {
                        break; // a newer generation replaced us; dropping `watcher` stops it
                    }
                    if dir {
                        town.talk(Word { kind: kind.clone(), tense: Tense::Past, body: Value::Null });
                    } else if let Ok(text) = std::fs::read_to_string(&path) {
                        town.talk(Word { kind: kind.clone(), tense: Tense::Present, body: Value::String(text) });
                    }
                }
            });
        }
    }

    // listen: each kind wakes the `talk` function; whatever it returns is talked
    let handler: Option<Function> = spec.get("talk")?;
    if let (Some(handler), Some(listen)) = (handler, spec.get::<Option<Table>>("listen")?) {
        for kind in listen.sequence_values::<String>().filter_map(Result::ok) {
            let handler = handler.clone();
            let town = town.clone();
            let lua = lua.clone();
            tokio::task::spawn_local(async move {
                let mut words = town.words.subscribe();
                // greet the resident with the present as it stands, so a fact already
                // said before it woke isn't lost
                let greeting = town.present.borrow().get(&kind).cloned();
                if let Some(body) = greeting {
                    say(&lua, &handler, &town, &Word { kind: kind.clone(), tense: Tense::Present, body });
                }
                loop {
                    if age < town.age.get() {
                        break; // a newer generation replaced us
                    }
                    match words.recv().await {
                        // re-check age AFTER waking: a reopen may have retired us WHILE we were
                        // parked in recv, and we must not dispatch one last word (double pickers).
                        Ok(w) if age >= town.age.get() && w.kind == kind => say(&lua, &handler, &town, &w),
                        Ok(_) => {}
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(_) => break,
                    }
                }
            });
        }
    }
    Ok(())
}

// Wake a resident with a word (a lua table {kind, tense, body}); talk whatever it says
// back. serde carries both directions: `Word` defaults tense to present and ignores the
// extra keys a resident attaches (label/act/aware). A `nil` return means "nothing to say"
// (the common case) and is silent; a Lua error, or a NON-nil return that isn't a word (a
// missing kind or a typo'd tense), is a bug — logged loudly rather than silently dropped.
fn say(lua: &Lua, handler: &Function, town: &Town, w: &Word) {
    let heard = match lua.to_value(w) {
        Ok(v) => v,
        Err(_) => return,
    };
    let said = match handler.call::<LuaValue>(heard) {
        Ok(v) => v,
        Err(e) => return eprintln!("town: resident errored handling '{}': {e}", w.kind),
    };
    if said.is_nil() {
        return; // said nothing — correct and common
    }
    match lua.from_value::<Word>(said) {
        Ok(word) => town.talk(word),
        Err(e) => eprintln!("town: resident returned a non-word handling '{}': {e}", w.kind),
    }
}
