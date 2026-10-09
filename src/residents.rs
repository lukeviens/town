//! The residents: townsfolk who listen to words and talk words back. Each returns a
//! { listen, talk } — the kinds it listens for, and a function whose return it talks.
//! A resident may also `watch` a file (talked in as a present word on change) or a
//! directory (a present word on any change within). `present(kind)`/`past(kind)` read
//! the town; `reopen()` re-reads every resident from disk, so town can hot-reload itself.

use std::rc::Rc;

use std::process::Stdio;

use mlua::{Function, Lua, LuaSerdeExt, Table, Value as LuaValue};
use notify::Watcher;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

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

    // every other citizen of town has `talk`; residents could only answer by returning
    let talker = town.clone();
    lua.globals().set(
        "talk",
        lua.create_function(move |lua, w: LuaValue| {
            match lua.from_value::<Word>(w) {
                Ok(word) => talker.talk(word),
                Err(e) => eprintln!("town: talk: not a word: {e}"),
            }
            Ok(())
        })?,
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
    // retire the previous generation's watch tasks — they can't reliably notice a reopen themselves
    for h in town.watchers.borrow_mut().drain(..) {
        h.abort();
    }
    town.joins.borrow_mut().clear();

    // the composition vocabulary first, so residents can be written with it
    if let Ok(src) = std::fs::read_to_string(paths::lib()) {
        if let Err(e) = lua.load(src).set_name("lib").exec() {
            eprintln!("town: lib: {e}");
        }
    }

    let mut files = Vec::new();
    walk(&paths::residents(), &mut files);
    files.sort(); // deterministic: a resident's listeners are wired before the next one's
    for path in files {
        if let Err(e) = wire(lua, town, &path, age) {
            eprintln!("town: {}: {e}", named(&path));
        }
    }
    Ok(())
}

/// Every resident under `dir`, however deep — a translator lives in residents/translators/, and
/// it is a resident like any other.
fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = e.path();
        if e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            walk(&p, out);
        } else if p.extension().and_then(|s| s.to_str()) == Some("lua") {
            out.push(p);
        }
    }
}

/// What a resident is called in an error: its stem, which is what its file is named.
fn named(path: &std::path::Path) -> String {
    path.file_stem().and_then(|s| s.to_str()).unwrap_or("?").to_string()
}

fn wire(lua: &Lua, town: &Rc<Town>, path: &std::path::Path, age: u64) -> mlua::Result<()> {
    let src = std::fs::read_to_string(path)?;
    let spec: Table = lua.load(src).set_name(named(path)).eval()?;

    // watch: a file talked in as its content, a directory as a bare change signal —
    // driven by real filesystem events (FSEvents/inotify), never a tick.
    if let Some(watch) = spec.get::<Option<Table>>("watch")? {
        for pair in watch.pairs::<String, String>() {
            let (kind, path) = pair?;
            let town_watch = town.clone();
            let dir = std::fs::metadata(&path).map(|m| m.is_dir()).unwrap_or(false);
            let handle = tokio::task::spawn_local(async move {
                let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
                let mut watcher = match notify::recommended_watcher(move |_| { let _ = tx.send(()); }) {
                    Ok(w) => w,
                    Err(_) => return,
                };
                let deep = if dir { notify::RecursiveMode::Recursive } else { notify::RecursiveMode::NonRecursive };
                if watcher.watch(std::path::Path::new(&path), deep).is_err() {
                    return;
                }
                // a file greets with its current content (so theme is set on boot); a directory doesn't
                if !dir {
                    if let Ok(text) = std::fs::read_to_string(&path) {
                        town_watch.talk(Word { kind: kind.clone(), tense: Tense::Present, body: Value::String(text), at: 0 });
                    }
                }
                while rx.recv().await.is_some() {
                    if dir {
                        town_watch.talk(Word { kind: kind.clone(), tense: Tense::Past, body: Value::Null, at: 0 });
                    } else if let Ok(text) = std::fs::read_to_string(&path) {
                        town_watch.talk(Word { kind: kind.clone(), tense: Tense::Present, body: Value::String(text), at: 0 });
                    }
                }
            });
            town.watchers.borrow_mut().push(handle);
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
                    say(&lua, &handler, &town, &Word { kind: kind.clone(), tense: Tense::Present, body, at: 0 });
                }
                loop {
                    if age < town.age.get() {
                        break; // a newer generation replaced us
                    }
                    match words.recv().await {
                        // re-check age AFTER waking: a reopen may have retired us WHILE we were
                        // parked in recv, and we must not dispatch one last word (double pickers).
                        // "*" listens to every kind
                        Ok(w) if age >= town.age.get() && (kind == "*" || w.kind == kind) => say(&lua, &handler, &town, &w),
                        Ok(_) => {}
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(_) => break,
                    }
                }
            });
        }
    }
    // join: a program town keeps talking to, the way `watch` is a file it keeps reading. Every
    // line the program says is a past word of the declared kind; `work { at = <kind> }` writes a
    // line back. What the lines MEAN is the resident's — the engine knows no protocol.
    if let Some(join) = spec.get::<Option<Table>>("join")? {
        for pair in join.pairs::<String, Table>() {
            let (kind, argv) = pair?;
            let argv: Vec<String> = argv.sequence_values::<String>().filter_map(Result::ok).collect();
            if argv.is_empty() {
                continue;
            }
            let town_join = town.clone();
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
            town.joins.borrow_mut().insert(kind.clone(), tx);
            let handle = tokio::task::spawn_local(async move {
                loop {
                    // kill_on_drop: a reopen aborts this task, and the program must go with it or
                    // every save would leave another one running.
                    let child = tokio::process::Command::new(&argv[0])
                        .args(&argv[1..])
                        .stdin(Stdio::piped())
                        .stdout(Stdio::piped())
                        .stderr(Stdio::null())
                        .kill_on_drop(true)
                        .spawn();
                    if let Ok(mut child) = child {
                        if let (Some(out), Some(mut inp)) = (child.stdout.take(), child.stdin.take()) {
                            // a bodiless word: the connection is open. Whatever the resident had
                            // said over the old one — a subscription, a mode — is gone with it.
                            town_join.talk(Word {
                                kind: kind.clone(),
                                tense: Tense::Past,
                                body: Value::Null,
                                at: 0,
                            });
                            let mut lines = BufReader::new(out).lines();
                            loop {
                                tokio::select! {
                                    line = lines.next_line() => match line {
                                        Ok(Some(l)) => town_join.talk(Word {
                                            kind: kind.clone(),
                                            tense: Tense::Past,
                                            body: Value::String(l),
                                            at: 0,
                                        }),
                                        _ => break, // it hung up
                                    },
                                    Some(cmd) = rx.recv() => {
                                        if inp.write_all(format!("{cmd}\n").as_bytes()).await.is_err() {
                                            break;
                                        }
                                    }
                                }
                            }
                        }
                    }
                    tokio::time::sleep(std::time::Duration::from_secs(2)).await; // join again
                }
            });
            town.watchers.borrow_mut().push(handle);
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
    // a null body is real nil in Lua, not mlua's truthy null sentinel: a resident testing
    // `if w.body == nil` must see what it plainly means.
    let opts = mlua::SerializeOptions::new()
        .serialize_none_to_null(false)
        .serialize_unit_to_null(false);
    let heard = match lua.to_value_with(w, opts) {
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
