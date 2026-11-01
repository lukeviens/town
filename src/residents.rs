//! The residents: a town's programs, Lua files that listen to words and talk words back. A file
//! that returns a resident — { kinds, listen } and perhaps `watch`, `join`, `own` — is wired; one
//! that returns nothing is a library, for the files after it. `present(kind)` and `past(kind)`
//! read the town. The files are opened again whenever one changes, so a town runs what is saved.

use std::cell::RefCell;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::rc::Rc;
use std::time::Duration;

use mlua::{Function, Lua, LuaSerdeExt, Table, Value as LuaValue};
use notify::{RecursiveMode, Watcher};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{ChildStdin, ChildStdout};
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::mpsc;

use crate::town::Town;
use crate::word::{Tense, Word};

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
                Err(e) => failed(&talker, format!("talked a non-word: {e}")),
            }
            Ok(())
        })?,
    )?;
    Ok(())
}

/// Read a town's programs, in the order given, and bring them to life: each path a Lua file, or a
/// folder of them: its own files in name order, then its folders. Bumps the town's age so the
/// previous generation of listeners retires; called once at boot and again whenever a program
/// changes. What failed is said last, once every new resident listens, so one may hear it.
pub fn open(lua: &Lua, town: &Rc<Town>, programs: &[PathBuf]) {
    town.age.set(town.age.get() + 1);
    let age = town.age.get();
    // retire the previous generation's watch tasks — they can't reliably notice a reopen themselves
    for h in town.watchers.borrow_mut().drain(..) {
        h.abort();
    }
    town.joins.borrow_mut().clear();
    *town.inside.borrow_mut() = Town::machinery();

    let mut failed = Vec::new();
    // how a resident is written, before any program, so every one can be written with it
    if let Err(e) = lua.load(crate::RESIDENT).set_name("resident").exec() {
        failed.push(("resident".to_string(), e.to_string()));
    }
    for path in files(programs) {
        if let Err(e) = wire(lua, town, &path, age) {
            failed.push((named(&path), e.to_string()));
        }
    }
    for (from, message) in failed {
        town.error(&from, None, message);
    }
}

/// The Lua files of a town's programs: each path as given, a folder as `walk` reads it. A file
/// named twice, by itself and in a folder, runs once, where it first comes.
fn files(programs: &[PathBuf]) -> Vec<PathBuf> {
    let (mut all, mut seen) = (Vec::new(), HashSet::new());
    for p in programs {
        if p.is_dir() {
            walk(p, &mut all, &mut seen);
        } else {
            all.push(p.clone());
        }
    }
    let mut ran = HashSet::new();
    all.retain(|f| ran.insert(f.canonicalize().unwrap_or(f.clone())));
    all
}

/// Open a town's programs again whenever one changes: a saved file runs a moment later. Each
/// folder is watched whole, and each folder a link leads to outside it; a made link may lead to
/// programs, so it opens the town again too. Each program path is watched through its folder,
/// since an editor may save by replacing it and a link may move.
pub fn keep(lua: &Lua, town: &Rc<Town>, programs: Vec<PathBuf>) {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let changed = tx.clone();
    let folders = notify::recommended_watcher(move |e: notify::Result<notify::Event>| {
        // a program, a link that may lead to some, or a folder or link taken away; never a hidden
        // file, which is no program
        let matters = |p: &PathBuf| {
            let hidden = p.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.'));
            let gone = !p.exists() && p.extension().is_none();
            !hidden && (p.is_symlink() || gone || p.extension().is_some_and(|x| x == "lua"))
        };
        if e.is_ok_and(|e| e.paths.iter().any(matters)) {
            changed.send(()).ok();
        }
    });
    let mut named: HashSet<PathBuf> = programs.iter().cloned().collect();
    named.extend(programs.iter().filter_map(|p| p.canonicalize().ok()));
    let paths: Vec<PathBuf> = named.iter().cloned().collect();
    let places = notify::recommended_watcher(move |e: notify::Result<notify::Event>| {
        if e.map(|e| e.paths.iter().any(|p| named.contains(p))).unwrap_or(false) {
            let _ = tx.send(());
        }
    });
    let (mut folders, mut places) = match (folders, places) {
        (Ok(f), Ok(p)) => (f, p),
        (Err(e), _) | (_, Err(e)) => {
            return town.error("town", None, format!("cannot watch its programs: {e}"))
        }
    };
    for at in paths.iter().filter_map(|p| p.parent()).collect::<HashSet<_>>() {
        if let Err(e) = places.watch(at, RecursiveMode::NonRecursive) {
            town.error("town", None, format!("cannot watch {}: {e}", at.display()));
        }
    }
    let mut watched = HashSet::new();
    rewatch(&mut folders, &mut watched, &programs, town);
    let (lua, town) = (lua.clone(), town.clone());
    tokio::task::spawn_local(async move {
        let _places = places; // held for as long as the town runs
        while rx.recv().await.is_some() {
            // one save, however many events it makes: they come a few ms apart, so open once
            // they stop, or a second after the first, if they never do
            let most = tokio::time::Instant::now() + Duration::from_secs(1);
            while let Ok(Some(())) =
                tokio::time::timeout_at((tokio::time::Instant::now() + SETTLE).min(most), rx.recv())
                    .await
            {}
            open(&lua, &town, &programs);
            rewatch(&mut folders, &mut watched, &programs, &town);
        }
    });
}

/// Watch the folders a town's programs are read from now, and stop watching those it no longer is:
/// each read folder that is not inside another, whole.
fn rewatch(
    watcher: &mut impl Watcher,
    watched: &mut HashSet<PathBuf>,
    programs: &[PathBuf],
    town: &Town,
) {
    let mut read = HashSet::new();
    for p in programs.iter().filter(|p| p.is_dir()) {
        walk(p, &mut Vec::new(), &mut read);
    }
    let want: HashSet<PathBuf> =
        read.iter().filter(|d| !d.ancestors().skip(1).any(|a| read.contains(a))).cloned().collect();
    for gone in watched.difference(&want) {
        watcher.unwatch(gone).ok();
    }
    let mut now: HashSet<PathBuf> = watched.intersection(&want).cloned().collect();
    for new in want.difference(watched) {
        match watcher.watch(new, RecursiveMode::Recursive) {
            Ok(()) => {
                now.insert(new.clone());
            }
            Err(e) => town.error("town", None, format!("cannot watch {}: {e}", new.display())),
        }
    }
    *watched = now;
}

/// Every Lua file under `dir`, however deep: a folder's own files in name order, then its folders,
/// so a library beside them comes first. A link is followed to what it names, once: `seen` holds
/// each folder read, as it truly is, so a link back up is not read again. A hidden file or folder
/// (`.git`, an editor's `.#lock`) is not a program.
fn walk(dir: &Path, out: &mut Vec<PathBuf>, seen: &mut HashSet<PathBuf>) {
    let Ok(real) = dir.canonicalize() else { return };
    if !seen.insert(real) {
        return;
    }
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
        .map(|e| e.path())
        .collect();
    entries.sort();
    let (folders, files): (Vec<PathBuf>, Vec<PathBuf>) =
        entries.into_iter().partition(|p| p.is_dir());
    out.extend(files.into_iter().filter(|p| p.extension().is_some_and(|x| x == "lua")));
    for f in folders {
        walk(&f, out, seen);
    }
}

/// What a resident is called in an error: its stem, which is what its file is named.
fn named(path: &Path) -> String {
    path.file_stem().and_then(|s| s.to_str()).unwrap_or("?").to_string()
}

fn wire(lua: &Lua, town: &Rc<Town>, path: &Path, age: u64) -> mlua::Result<()> {
    let src = std::fs::read_to_string(path)?;
    let spec: Option<Table> = lua.load(src).set_name(named(path)).eval()?;
    let Some(spec) = spec else {
        return Ok(()); // a library: what it defined is there for the files after it
    };
    let from = named(path);

    // watch: a file or folder town keeps reading. It speaks for the file, not for anyone. A
    // relative path is beside the resident, not wherever the town was started.
    if let Some(watch) = spec.get::<Option<Table>>("watch")? {
        let here = path.parent().unwrap_or(Path::new("/"));
        for pair in watch.pairs::<String, String>() {
            let (kind, file) = pair?;
            let file = here.join(file).to_string_lossy().into_owned();
            town.inside.borrow_mut().insert(kind.clone());
            let task = tokio::task::spawn_local(watching(town.clone(), from.clone(), kind, file));
            town.watchers.borrow_mut().push(task);
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
            town.inside.borrow_mut().insert(kind);
        }
    }

    // join: a program town keeps talking to, the way `watch` is a file it keeps reading. What its
    // lines mean is the resident's: the engine knows no protocol. It speaks only through town.
    if let Some(join) = spec.get::<Option<Table>>("join")? {
        for pair in join.pairs::<String, Table>() {
            let (kind, argv) = pair?;
            let argv: Vec<String> = argv.sequence_values().filter_map(Result::ok).collect();
            if argv.is_empty() {
                continue;
            }
            let (tx, rx) = mpsc::unbounded_channel();
            town.joins.borrow_mut().insert(kind.clone(), tx);
            town.inside.borrow_mut().insert(kind.clone());
            let task =
                tokio::task::spawn_local(joining(town.clone(), from.clone(), kind, argv, rx));
            town.watchers.borrow_mut().push(task);
        }
    }
    Ok(())
}

/// How quiet a save must fall before the town opens again: its events come a few ms apart.
const SETTLE: Duration = Duration::from_millis(50);

/// A watched file, said as a fact of its content: as the town opens, so its resident is set, and
/// then whenever the content changes, since one save can make several events. A file that is gone
/// is said as a fact of nothing. A folder is said as a bare change. Real filesystem events
/// (FSEvents, inotify), never a tick. A save that takes the file away a moment is said as it
/// happened, gone and then back; a resident that would rather not see the gap waits it out itself
/// (`after`).
async fn watching(town: Rc<Town>, from: String, kind: String, path: String) {
    let file = PathBuf::from(&path);
    let dir = file.is_dir();
    // a file through its folder: a save that replaces it would end a watch on the file itself
    let (at, name) = match (dir, file.parent(), file.file_name()) {
        (false, Some(folder), Some(name)) => (folder.to_path_buf(), Some(name.to_os_string())),
        _ => (file.clone(), None),
    };
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut watcher = match notify::recommended_watcher(move |e: notify::Result<notify::Event>| {
        let ours = |p: &PathBuf| name.as_ref().is_none_or(|n| p.file_name() == Some(n));
        if e.is_ok_and(|e| e.paths.iter().any(ours)) {
            tx.send(()).ok();
        }
    }) {
        Ok(w) => w,
        Err(e) => return town.error(&from, None, format!("cannot watch {path}: {e}")),
    };
    let deep = if dir { RecursiveMode::Recursive } else { RecursiveMode::NonRecursive };
    if let Err(e) = watcher.watch(&at, deep) {
        return town.error(&from, None, format!("cannot watch {path}: {e}"));
    }
    if dir {
        while rx.recv().await.is_some() {
            town.talk(Word::new(&kind, Tense::Past, Value::Null));
        }
        return;
    }
    // what the present already holds was heard in the greeting: said again only if it differs
    let mut said = town.present.borrow().get(&kind).and_then(|v| v.as_str().map(String::from));
    let mut known = town.present.borrow().contains_key(&kind);
    loop {
        match std::fs::read_to_string(&path) {
            Ok(text) if said.as_ref() != Some(&text) => {
                said = Some(text.clone());
                known = true;
                town.talk(Word::new(&kind, Tense::Present, Value::String(text)));
            }
            // only a file that is not there is gone: one that cannot be read keeps what it said
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && known => {
                said = None;
                known = false;
                town.talk(Word::new(&kind, Tense::Present, Value::Null));
            }
            _ => {}
        }
        if rx.recv().await.is_none() {
            return;
        }
    }
}

/// One kind a resident hears: the present of it first (all of it, for "*"), then each word of it,
/// until the town opens again. It listens from now, not from when its task first runs, so what
/// opening the town says after this, its failures among them, is heard; and its greeting is the
/// present as of now, so a fact is heard once: said before now, in the greeting; after, as a word.
fn hearing(lua: &Lua, town: &Rc<Town>, from: &str, handler: &Function, kind: String, age: u64) {
    let mut words = town.words.subscribe();
    let greeting: Vec<Word> =
        town.present_words().into_iter().filter(|w| kind == "*" || w.kind == kind).collect();
    let (lua, town, from, handler) = (lua.clone(), town.clone(), from.to_string(), handler.clone());
    tokio::task::spawn_local(async move {
        for w in greeting {
            listen(&lua, &handler, &town, &from, &w);
        }
        // a newer generation retires this one: asked again on waking, since a reopen may come
        // while it waits, and a retired one must not take one last word. "*" hears every kind.
        while age >= town.age.get() {
            match words.recv().await {
                Ok(w) if age >= town.age.get() && (kind == "*" || w.kind == kind) => {
                    listen(&lua, &handler, &town, &from, &w)
                }
                Ok(_) | Err(RecvError::Lagged(_)) => {}
                Err(RecvError::Closed) => break,
            }
        }
    });
}

/// A joined program, kept running: started again two seconds after it ends. It is said as a
/// bodiless word each time it starts, since whatever a resident said to the one before — a
/// subscription, a mode — is gone with it.
async fn joining(
    town: Rc<Town>,
    from: String,
    kind: String,
    argv: Vec<String>,
    mut lines_in: mpsc::UnboundedReceiver<String>,
) {
    let mut told = false; // a program that cannot start is said once, not every try
    loop {
        // kill_on_drop: a reopen aborts this task, and the program must go with it, or every save
        // would leave another one running
        let child = tokio::process::Command::new(&argv[0])
            .args(&argv[1..])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn();
        match child {
            Err(e) if !told => {
                told = true;
                town.error(&from, None, format!("cannot join {}: {e}", argv[0]));
            }
            Err(_) => {}
            Ok(mut child) => {
                told = false;
                if let (Some(out), Some(inp)) = (child.stdout.take(), child.stdin.take()) {
                    town.talk(Word::new(&kind, Tense::Past, Value::Null));
                    converse(&town, &kind, out, inp, &mut lines_in).await;
                }
            }
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

/// Lines both ways with a joined program, until it hangs up: each it says is a past word of
/// `kind`, and each `work` sends it is written to it.
async fn converse(
    town: &Town,
    kind: &str,
    out: ChildStdout,
    mut inp: ChildStdin,
    lines_in: &mut mpsc::UnboundedReceiver<String>,
) {
    let mut lines = BufReader::new(out).lines();
    loop {
        tokio::select! {
            line = lines.next_line() => match line {
                Ok(Some(line)) => town.talk(Word::new(kind, Tense::Past, Value::String(line))),
                _ => return,
            },
            Some(line) = lines_in.recv() => {
                if inp.write_all(format!("{line}\n").as_bytes()).await.is_err() {
                    return;
                }
            }
        }
    }
}

// A resident listens to a word (a lua table {kind, tense, body}); talk whatever it says
// back. serde carries both directions: `Word` defaults tense to present and ignores the
// extra keys a resident attaches (label/act/aware). A `nil` return means "nothing to say"
// (the common case) and is silent; a Lua error, or a NON-nil return that isn't a word (a
// missing kind or a typo'd tense), is a bug — said as an `error` word rather than dropped.
fn listen(lua: &Lua, handler: &Function, town: &Town, from: &str, w: &Word) {
    HEARING.with(|h| *h.borrow_mut() = Some((from.to_string(), w.kind.clone())));
    hear(lua, handler, town, w);
    HEARING.with(|h| *h.borrow_mut() = None);
}

fn hear(lua: &Lua, handler: &Function, town: &Town, w: &Word) {
    let fail = |message: String| failed(town, message);
    // a null body is real nil in Lua, not mlua's truthy null sentinel: a resident testing
    // `if w.body == nil` must see what it plainly means.
    let opts =
        mlua::SerializeOptions::new().serialize_none_to_null(false).serialize_unit_to_null(false);
    let heard = match lua.to_value_with(w, opts) {
        Ok(v) => v,
        Err(_) => return,
    };
    let said = match handler.call::<LuaValue>(heard) {
        Ok(v) => v,
        Err(e) => return fail(e.to_string()),
    };
    if said.is_nil() {
        return; // said nothing — correct and common
    }
    match lua.from_value::<Word>(said) {
        Ok(word) => town.talk(word),
        Err(e) => fail(format!("answered with a non-word: {e}")),
    }
}

#[cfg(test)]
mod inside {
    use super::*;
    use crate::word::Tense;

    fn word(kind: &str) -> Word {
        Word::new(kind, Tense::Future, Value::Null)
    }

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
            town.talk(Word::new("ping", Tense::Past, Value::Null));
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
            lua.load(crate::RESIDENT).exec().unwrap();
            let mut words = town.words.subscribe();
            for ms in ["-1", "'500'", "0/0", "math.huge"] {
                let asks = format!("function(w) return after({ms}, {{ about = 'x' }}) end");
                let asks = lua.load(asks).eval::<Function>().unwrap();
                listen(&lua, &asks, &town, "waiter", &word("ping"));
                let said = words.try_recv().expect("the bad wait is a word");
                assert_eq!(
                    (said.kind.as_str(), said.body["from"].as_str()),
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
            assert_eq!(said.body["from"], "broken");
            assert_eq!(said.body["heard"], "ping");
            assert!(said.body["message"].as_str().unwrap().contains("no ping"));

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
                (said.kind.as_str(), said.body["from"].as_str(), said.body["heard"].as_str()),
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
                r#"local n = 0
                return {{ kinds = {{ "colors" }}, {watch},
                  listen = function() n = n + 1; return event("heard", n) end }}"#
            );
            std::fs::write(&resident, lua).unwrap();
            let town = Town::with_log(dir.join("past.log"));
            let lua = Lua::new();
            install(&lua, &town).unwrap();
            let mut words = town.words.subscribe();
            open(&lua, &town, std::slice::from_ref(&resident));
            let mut heard = Vec::new();
            for _ in 0..20 {
                tokio::task::yield_now().await;
                while let Ok(w) = words.try_recv() {
                    if w.kind == "heard" {
                        heard.push(w.body)
                    }
                }
            }
            assert_eq!(heard, [serde_json::json!(1)]);
            for h in town.watchers.borrow_mut().drain(..) {
                h.abort();
            }
            std::fs::remove_dir_all(&dir).ok();
        });
    }

    // a watched file already kept in the present is heard once as the town opens, in the greeting;
    // a save that replaces the file, as `emit` does, is still seen, each time
    #[test]
    fn a_kept_file_is_heard_once_and_every_save_after() {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        tokio::task::LocalSet::new().block_on(&rt, async {
            let dir = std::env::temp_dir().join(format!("town-kept-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            let file = dir.join("colors");
            std::fs::write(&file, "bg=#000").unwrap();
            let kept = Word::new("colors", Tense::Present, Value::String("bg=#000".into()));
            std::fs::write(dir.join("past.log"), kept.to_line() + "\n").unwrap();
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
            let mut heard = Vec::new();
            let mut listen = async |ms| {
                tokio::time::sleep(Duration::from_millis(ms)).await;
                while let Ok(w) = words.try_recv() {
                    if w.kind == "heard" {
                        heard.push(w.body.as_str().unwrap_or("").to_string())
                    }
                }
            };
            listen(300).await;
            for bg in ["bg=#111", "bg=#222"] {
                std::fs::write(dir.join("colors.writing"), bg).unwrap();
                std::fs::rename(dir.join("colors.writing"), &file).unwrap();
                listen(300).await;
            }
            assert_eq!(heard, ["bg=#000", "bg=#111", "bg=#222"]);
            for h in town.watchers.borrow_mut().drain(..) {
                h.abort();
            }
            std::fs::remove_dir_all(&dir).ok();
        });
    }

    // a watched file that is gone is said as a fact of nothing, and said again when it is back
    #[test]
    fn a_gone_file_is_said_as_nothing() {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        tokio::task::LocalSet::new().block_on(&rt, async {
            let dir = std::env::temp_dir().join(format!("town-gone-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            let file = dir.join("colors");
            std::fs::write(&file, "bg=#000").unwrap();
            let resident = dir.join("r.lua");
            std::fs::write(&resident, r#"return { watch = { colors = "colors" } }"#).unwrap();
            let town = Town::with_log(dir.join("past.log"));
            let mut words = town.words.subscribe();
            wire(&Lua::new(), &town, &resident, 0).unwrap();
            let mut said = Vec::new();
            let mut wait = async |ms| {
                tokio::time::sleep(Duration::from_millis(ms)).await;
                while let Ok(w) = words.try_recv() {
                    said.push(w.body);
                }
            };
            wait(200).await;
            std::fs::remove_file(&file).unwrap();
            wait(400).await;
            assert!(!town.present.borrow().contains_key("colors"), "a gone file is not present");
            std::fs::write(&file, "bg=#fff").unwrap();
            wait(400).await;
            // an editor's save that takes the file away a moment: whether the gap is seen is the
            // filesystem's to say, but what the file holds after it always is
            std::fs::remove_file(&file).unwrap();
            wait(120).await;
            std::fs::write(&file, "bg=#abc").unwrap();
            wait(400).await;
            let bg = |s: &str| Value::String(s.into());
            assert_eq!(said[..3], [bg("bg=#000"), Value::Null, bg("bg=#fff")]);
            assert_eq!(said.last(), Some(&bg("bg=#abc")));
            for h in town.watchers.borrow_mut().drain(..) {
                h.abort();
            }
            std::fs::remove_dir_all(&dir).ok();
        });
    }

    // a relative watch path is beside the resident, wherever the town was started
    #[test]
    fn a_watch_path_is_beside_its_resident() {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        tokio::task::LocalSet::new().block_on(&rt, async {
            let dir = std::env::temp_dir().join(format!("town-beside-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("colors"), "bg=#000").unwrap();
            let resident = dir.join("r.lua");
            std::fs::write(&resident, r#"return { watch = { colors = "colors" } }"#).unwrap();
            let town = Town::with_log(dir.join("past.log"));
            let mut words = town.words.subscribe();
            wire(&Lua::new(), &town, &resident, 0).unwrap();
            let said = async {
                for _ in 0..10 {
                    tokio::task::yield_now().await;
                    if let Ok(w) = words.try_recv() {
                        return Some(w);
                    }
                }
                None
            };
            let said = said.await.expect("the file was said");
            assert_eq!(
                (said.kind.as_str(), &said.body),
                ("colors", &Value::String("bg=#000".into()))
            );
            for h in town.watchers.borrow_mut().drain(..) {
                h.abort();
            }
            std::fs::remove_dir_all(&dir).ok();
        });
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
            town.talk(Word::new("theme", Tense::Present, Value::String("dark".into())));
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
            assert_eq!(heard.await, Some(Value::String("theme".into())));
            std::fs::remove_dir_all(&dir).ok();
        });
    }

    // a clause with no kind matches every kind, so the resident hears every kind
    #[test]
    fn a_kindless_clause_hears_every_kind() {
        let lua = Lua::new();
        lua.load(crate::RESIDENT).exec().unwrap();
        let kinds: Vec<String> = lua
            .load(r#"return react{ on("a", print), on({ tense = "past" }, print) }.kinds"#)
            .eval::<Vec<String>>()
            .unwrap();
        assert_eq!(kinds, ["*"]);
    }

    // a program that cannot start is said, once
    #[test]
    fn a_join_that_cannot_start_is_an_error_word() {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        tokio::task::LocalSet::new().block_on(&rt, async {
            let dir = std::env::temp_dir().join(format!("town-joinfail-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            let resident = dir.join("bridge.lua");
            std::fs::write(&resident, "return { join = { echo = { \"/no/such/program\" } } }")
                .unwrap();
            let town = Town::with_log(dir.join("past.log"));
            let mut words = town.words.subscribe();
            wire(&Lua::new(), &town, &resident, 0).unwrap();
            tokio::task::yield_now().await;
            let said = words.try_recv().expect("the failure is a word");
            assert_eq!((said.kind.as_str(), said.body["from"].as_str()), ("error", Some("bridge")));
            assert!(said.body["message"].as_str().unwrap().contains("/no/such/program"));
            for h in town.watchers.borrow_mut().drain(..) {
                h.abort();
            }
            std::fs::remove_dir_all(&dir).ok();
        });
    }

    // a folder's own files come before its folders, so a library beside them is there first; a
    // linked folder is read like any other, once, so a link back up ends
    #[test]
    fn a_folder_reads_its_files_then_its_folders() {
        let dir = std::env::temp_dir().join(format!("town-walk-{}", std::process::id()));
        let shared = dir.join("shared");
        std::fs::create_dir_all(dir.join("town/apps")).unwrap();
        std::fs::create_dir_all(&shared).unwrap();
        for f in ["town/lib.lua", "town/apps/echo.lua", "town/notes.txt", "shared/linked.lua"] {
            std::fs::write(dir.join(f), "").unwrap();
        }
        std::os::unix::fs::symlink(&shared, dir.join("town/b-linked")).unwrap();
        std::os::unix::fs::symlink(dir.join("town"), dir.join("town/apps/up")).unwrap();
        std::fs::create_dir_all(dir.join("town/.git")).unwrap();
        std::fs::write(dir.join("town/.git/hidden.lua"), "").unwrap();
        std::fs::write(dir.join("town/.#lib.lua"), "").unwrap();
        let names: Vec<String> = files(&[dir.join("town")])
            .iter()
            .map(|p| p.strip_prefix(dir.join("town")).unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["lib.lua", "apps/echo.lua", "b-linked/linked.lua"]);
        // a file named by itself and in its folder runs once, where it first comes
        let twice = files(&[dir.join("town/apps/echo.lua"), dir.join("town")]);
        assert_eq!(twice.len(), 3);
        assert!(twice[0].ends_with("apps/echo.lua"));
        std::fs::remove_dir_all(&dir).ok();
    }
}
