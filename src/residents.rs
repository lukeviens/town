//! The residents: a town's programs, files that listen to words and talk words back. A Lua file
//! that returns a resident — { kinds, listen } and perhaps `watch` and `own` — is wired; one
//! that returns nothing is a library, for the files after it. `present(kind)` and `past(kind)`
//! read the town. A file town runs, in any language, is a resident too: it says what it is on its
//! first line, then words, a line each, both ways. The files are opened again whenever one
//! changes, so a town runs what is saved.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::rc::Rc;
use std::time::Duration;

use mlua::{Function, Lua, LuaSerdeExt, Table, Value as LuaValue};
use notify::{RecursiveMode, Watcher};
use serde_json::Value;
use tokio::io::AsyncWriteExt;
use tokio::process::{Child, ChildStdin, ChildStdout};
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::mpsc;

use crate::town::{Lines, Town};
use crate::unix;
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

/// Read a town's programs, in the order given, and bring them to life: each path a program, or a
/// folder of them: its own files in name order, then its folders. Bumps the town's age so the
/// previous generation of listeners retires; called once at boot and again whenever a program
/// changes. What failed is said last, once every new resident listens, so one may hear it.
pub fn open(lua: &Lua, town: &Rc<Town>, programs: &[PathBuf]) {
    open_read(lua, town, &files(programs).files);
}

/// `open`, with the programs already read.
fn open_read(lua: &Lua, town: &Rc<Town>, files: &[PathBuf]) {
    town.age.set(town.age.get() + 1);
    let age = town.age.get();
    // retire the previous generation's tasks, its watches, joins and programs: they cannot reliably
    // notice a reopen themselves
    for h in town.tasks.borrow_mut().drain(..) {
        h.abort();
    }
    // and end their programs now, not as the tasks are dropped: a new run must not meet the old
    end_groups(town);
    town.joins.borrow_mut().clear();
    *town.inside.borrow_mut() = Town::machinery();

    let mut failed = Vec::new();
    // how a resident is written, before any program, so every one can be written with it
    if let Err(e) = lua.load(crate::RESIDENT).set_name("resident").exec() {
        failed.push(("resident".to_string(), e.to_string()));
    }
    for path in files {
        if runnable(path) {
            let task = tokio::task::spawn_local(residing(town.clone(), path.clone()));
            town.tasks.borrow_mut().push(task);
        } else if let Err(e) = wire(lua, town, path, age) {
            failed.push((named(path), e.to_string()));
        }
    }
    for (from, message) in failed {
        town.error(&from, None, message);
    }
}

/// A program of a town: a Lua file, or a file it runs, wherever it is in the town's folders. What a
/// folder holds is the town, so a build is kept outside it and its program linked in. Never a
/// hidden file (`.git`, an editor's `.#lock`), nor an editor's backup (`name~`).
fn program(path: &Path) -> bool {
    program_named(path) && (path.extension().is_some_and(|x| x == "lua") || runnable(path))
}

/// Whether a file's name may be a program's: not hidden, nor an editor's backup.
fn program_named(path: &Path) -> bool {
    let name = path.file_name().map(|n| n.to_string_lossy()).unwrap_or_default();
    !name.starts_with('.') && !name.ends_with('~')
}

/// A program town runs rather than reads: a file it may execute that is not Lua.
fn runnable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.extension().is_none_or(|x| x != "lua")
        && path.metadata().is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

/// What a town is read from: its programs, in the order they run; the folders read, as they truly
/// are; and each link to a file among them, whether it leads anywhere yet or not.
#[derive(Default)]
struct Read {
    files: Vec<PathBuf>,
    folders: HashSet<PathBuf>,
    links: Vec<PathBuf>,
}

/// The programs of a town: each path as given, a folder as `walk` reads it. A file named twice, by
/// itself and in a folder, runs once, where it first comes.
fn files(programs: &[PathBuf]) -> Read {
    let mut read = Read::default();
    for p in programs {
        if p.is_dir() {
            walk(p, &mut read);
        } else {
            read.files.push(p.clone());
            read.links.extend(p.is_symlink().then(|| p.clone()));
        }
    }
    let mut ran = HashSet::new();
    read.files.retain(|f| ran.insert(f.canonicalize().unwrap_or(f.clone())));
    read
}

/// How a town's programs look: where each truly is, when it was saved, its size and its mode. The
/// town opens again when this changes, and only then.
#[derive(PartialEq)]
struct Look {
    at: PathBuf,
    saved: Option<std::time::SystemTime>,
    size: u64,
    mode: u32,
}

fn look(files: &[PathBuf]) -> Vec<Look> {
    use std::os::unix::fs::PermissionsExt;
    let look = |f: &PathBuf| {
        let m = f.metadata().ok();
        Look {
            at: f.canonicalize().unwrap_or(f.clone()),
            saved: m.as_ref().and_then(|m| m.modified().ok()),
            size: m.as_ref().map_or(0, |m| m.len()),
            mode: m.map_or(0, |m| m.permissions().mode()),
        }
    };
    files.iter().map(look).collect()
}

/// Open a town's programs, and again whenever one changes: a saved file runs a moment later. Each
/// folder is watched whole, and each folder a link leads to. A link to a file is watched where it
/// leads, or as near as there is yet, so a program linked from a build beside the town runs again
/// when it is built. Each path given is watched through its folder, since an editor may save by
/// replacing it and a link may move.
pub fn keep(lua: &Lua, town: &Rc<Town>, programs: Vec<PathBuf>) {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let (changed, again) = (tx.clone(), tx.clone());
    // any change in what is read: whether it is one to the programs is for `look` to say
    let folders = notify::recommended_watcher(move |e: notify::Result<notify::Event>| {
        if e.is_ok() {
            changed.send(()).ok();
        }
    });
    // the folders that hold the paths given hold much else: only a change to a path given counts
    let mut given: HashSet<PathBuf> = programs.iter().cloned().collect();
    given.extend(programs.iter().filter_map(|p| p.canonicalize().ok()));
    let holders: HashSet<PathBuf> =
        given.iter().filter_map(|p| p.parent()).map(Path::to_path_buf).collect();
    let places = notify::recommended_watcher(move |e: notify::Result<notify::Event>| {
        if e.is_ok_and(|e| e.paths.iter().any(|p| given.contains(p))) {
            tx.send(()).ok();
        }
    });
    let (mut folders, mut places) = match (folders, places) {
        (Ok(f), Ok(p)) => (f, p),
        (Err(e), _) | (_, Err(e)) => {
            // a town that cannot see its changes still runs what it has
            open(lua, town, &programs);
            return town.error("town", None, format!("cannot watch its programs: {e}"));
        }
    };
    for at in &holders {
        if let Err(e) = places.watch(at, RecursiveMode::NonRecursive) {
            town.error("town", None, format!("cannot watch {}: {e}", at.display()));
        }
    }
    let mut watched = HashMap::new();
    let read = files(&programs);
    // looked at before it is read, so a save while the town opens is a change after
    let mut was = look(&read.files);
    open_read(lua, town, &read.files);
    rewatch(&mut folders, &mut watched, &read, town);
    // and looked at once more, now it is watched: a save before the watches began made no event
    again.send(()).ok();
    let (lua, town) = (lua.clone(), town.clone());
    tokio::task::spawn_local(async move {
        let _places = places; // held for as long as the town runs
        while rx.recv().await.is_some() {
            // one save, however many events it makes: they come a few ms apart, so look once
            // they stop, or a second after the first, if they never do
            let most = tokio::time::Instant::now() + Duration::from_secs(1);
            while let Ok(Some(())) =
                tokio::time::timeout_at((tokio::time::Instant::now() + SETTLE).min(most), rx.recv())
                    .await
            {}
            let read = files(&programs);
            let now = look(&read.files);
            if now != was {
                was = now;
                open_read(&lua, &town, &read.files);
            }
            // a link made may lead to a folder with no program in it yet
            rewatch(&mut folders, &mut watched, &read, &town);
        }
    });
}

/// Where a link to a file leads: the folder it is in, or the nearest folder there is on the way to
/// it, if it is not there yet.
fn leads(link: &Path) -> Option<PathBuf> {
    let to = link.parent()?.join(std::fs::read_link(link).ok()?);
    to.ancestors().skip(1).find_map(|a| a.canonicalize().ok())
}

/// Watch what a town is read from now, and stop watching what it no longer is: each folder read
/// that is not inside another, whole; and where each link to a file leads, if no such folder holds
/// it, alone. One that cannot be watched is said once, not tried again at every change.
fn rewatch(
    watcher: &mut impl Watcher,
    watched: &mut HashMap<PathBuf, Watch>,
    read: &Read,
    town: &Town,
) {
    use std::os::unix::fs::MetadataExt;
    let which = |d: &Path| std::fs::metadata(d).ok().map(|m| m.ino());
    let inside = |d: &Path, of: &HashSet<PathBuf>| d.ancestors().skip(1).any(|a| of.contains(a));
    let whole: HashSet<PathBuf> =
        read.folders.iter().filter(|d| !inside(d, &read.folders)).cloned().collect();
    let mut want: HashMap<PathBuf, Watch> =
        whole.iter().map(|d| (d.clone(), (RecursiveMode::Recursive, which(d)))).collect();
    for at in read.links.iter().filter_map(|l| leads(l)) {
        if !whole.contains(&at) && !inside(&at, &whole) {
            let watch = (RecursiveMode::NonRecursive, which(&at));
            want.insert(at, watch);
        }
    }
    watched.retain(|d, watch| {
        let keep = want.get(d) == Some(watch);
        if !keep {
            watcher.unwatch(d).ok();
        }
        keep
    });
    for (d, watch) in want {
        if watched.contains_key(&d) {
            continue;
        }
        if let Err(e) = watcher.watch(&d, watch.0) {
            town.error("town", None, format!("cannot watch {}: {e}", d.display()));
        }
        watched.insert(d, watch);
    }
}

/// How a folder is watched, and which folder it is: one made again in its place is another, and
/// is watched anew, since a watch may end with the folder it was on (inotify's does).
type Watch = (RecursiveMode, Option<u64>);

/// Every program under `dir`, however deep: a folder's own files in name order, then its folders,
/// so a library beside them comes first. A link is followed to what it names, once: `seen` holds
/// each folder read, as it truly is, so a link back up is not read again. A hidden folder is not
/// read.
fn walk(dir: &Path, read: &mut Read) {
    let Ok(real) = dir.canonicalize() else { return };
    if !read.folders.insert(real) {
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
    read.links.extend(files.iter().filter(|p| p.is_symlink() && program_named(p)).cloned());
    read.files.extend(files.into_iter().filter(|p| program(p)));
    for f in folders {
        walk(&f, read);
    }
}

/// What a resident is called: its file's name, but for `.lua`.
fn named(path: &Path) -> String {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or("?".into());
    name.strip_suffix(".lua").map(String::from).unwrap_or(name)
}

fn wire(lua: &Lua, town: &Rc<Town>, path: &Path, age: u64) -> mlua::Result<()> {
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
        for pair in watch.pairs::<String, String>() {
            let (kind, file) = pair?;
            let task = watch_for(town, &from, path, kind, &file);
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

/// watch: a file or folder town keeps reading for a resident. It speaks for the file, not for
/// anyone. A relative path is beside the resident, not wherever the town was started.
fn watch_for(
    town: &Rc<Town>,
    from: &str,
    resident: &Path,
    kind: String,
    file: &str,
) -> tokio::task::JoinHandle<()> {
    let file = resident.parent().unwrap_or(Path::new("/")).join(file);
    town.hold(&kind);
    tokio::task::spawn_local(watching(
        town.clone(),
        from.to_string(),
        kind,
        file.to_string_lossy().into(),
    ))
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
        town.present_words().into_iter().filter(|w| hears(&kind, &w.kind)).collect();
    let (lua, town, from, handler) = (lua.clone(), town.clone(), from.to_string(), handler.clone());
    tokio::task::spawn_local(async move {
        for w in greeting {
            listen(&lua, &handler, &town, &from, &w);
        }
        // a newer generation retires this one: asked again on waking, since a reopen may come
        // while it waits, and a retired one must not take one last word
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

/// Whether a resident that hears `kind` hears a word of kind `said`. "*" is every kind but
/// `error`, which is named to be heard: two that hear everything, and fail at what they hear,
/// would otherwise answer each other's failings forever.
fn hears(kind: &str, said: &str) -> bool {
    kind == said || (kind == "*" && said != "error")
}

/// What a runnable resident says first, to be one: a Lua resident's table, as a line of JSON.
/// `kinds` it hears; perhaps `watch` and `own`, as a Lua resident has them. Nothing else: a
/// misspelled field would hear nothing, and say nothing of why.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Declaration {
    #[serde(default)]
    kinds: Vec<String>,
    #[serde(default)]
    watch: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    own: Vec<String>,
}

/// A runnable resident, run once in its own folder, until it ends or the town opens again. One
/// that fails says how, and stays ended, as one that ends well does: to run it again is to save it
/// again.
async fn residing(town: Rc<Town>, path: PathBuf) {
    let from = named(&path);
    // a group of its own, so all it starts ends with it (`Group`); kill_on_drop reaps it
    let child = tokio::process::Command::new(&path)
        .current_dir(path.parent().unwrap_or(Path::new("/")))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .kill_on_drop(true)
        .spawn();
    match child {
        Err(e) => town.error(&from, None, format!("cannot run: {e}")),
        Ok(child) => reside(&town, &from, &path, child).await,
    }
}

/// A run of a runnable resident: `live`, and each line on its stderr an `error`, every one heard,
/// though it said them as it ended.
async fn reside(town: &Rc<Town>, from: &str, path: &Path, mut child: Child) {
    let group = Group::of(town, &child);
    let (Some(out), Some(inp), Some(err)) =
        (child.stdout.take(), child.stdin.take(), child.stderr.take())
    else {
        return;
    };
    let mut stderr = Tasks(vec![tokio::task::spawn_local({
        let (town, from) = (town.clone(), from.to_string());
        async move {
            let mut lines = Lines::new(err);
            while let Some(line) = lines.next().await {
                town.error(&from, None, line);
            }
        }
    })]);
    live(town, from, path, &mut child, out, inp).await;
    // all it started ends with it, so its stderr closes, and the last of it is heard
    drop(group);
    for h in &mut stderr.0 {
        h.await.ok();
    }
}

/// A run, from its first line to its end. Its first line declares it; then it hears the present
/// of its kinds and each word of them since it started, and what it says is said in town. What it
/// watches and owns is its own while it runs.
async fn live(
    town: &Rc<Town>,
    from: &str,
    path: &Path,
    child: &mut Child,
    out: ChildStdout,
    mut inp: ChildStdin,
) {
    // heard from the moment it starts, not from when it has said what it is: the present as of
    // now, then every word after, so a fact is heard once
    let (mut words, greeting) = (town.words.subscribe(), town.present_words());
    // written aside, so a program slow to read never stops it being heard; but only so much
    let (tx, mut rx) = mpsc::channel::<Word>(1024);
    let mut tasks = Tasks(vec![tokio::task::spawn_local(async move {
        while let Some(w) = rx.recv().await {
            if inp.write_all(format!("{}\n", w.to_line()).as_bytes()).await.is_err() {
                return;
            }
        }
    })]);
    let mut lines = Lines::new(out);
    let Some(first) = lines.next().await else {
        if let Err(how) = how(child.wait().await) {
            town.error(from, None, format!("ended before it said what it is: {how}"));
        }
        return;
    };
    let declaration = match serde_json::from_str::<Declaration>(&first) {
        Ok(d) => d,
        Err(e) => {
            town.error(from, None, format!("its first line must say what it is: {e}"));
            return;
        }
    };
    let listens = |said: &str| declaration.kinds.iter().any(|kind| hears(kind, said));
    let mut hand = Hand { tx, told: false };
    for w in greeting.into_iter().filter(|w| listens(&w.kind)) {
        hand.over(town, from, w);
    }
    // the kinds it made its own, held while it runs, given back as it ends
    let own: Vec<String> =
        declaration.own.iter().chain(declaration.watch.keys()).cloned().collect();
    declaration.own.iter().for_each(|kind| town.hold(kind));
    for (kind, file) in declaration.watch {
        tasks.0.push(watch_for(town, from, path, kind, &file));
    }
    let mut talking = true;
    loop {
        tokio::select! {
            // what it said before it ended is heard before its end
            biased;
            line = lines.next(), if talking => match line {
                Some(line) => {
                    if let Some(w) = town.hear(from, &line) {
                        town.talk(w);
                    }
                }
                None => talking = false,
            },
            w = words.recv() => match w {
                Ok(w) => {
                    // never its own failings: one it hears and says again would be heard again
                    let its_own = w.kind == "error" && w.body["from"] == from;
                    if listens(&w.kind) && !its_own {
                        hand.over(town, from, w);
                    }
                }
                Err(RecvError::Lagged(n)) => {
                    town.error(from, None, format!("missed {n} words: it fell behind"))
                }
                Err(RecvError::Closed) => break,
            },
            status = child.wait() => {
                if let Err(how) = how(status) {
                    town.error(from, None, format!("ended: {how}"));
                }
                break;
            }
        }
    }
    own.iter().for_each(|kind| town.free(kind));
}

/// Whether a run ended well, and if not, how it ended.
fn how(status: std::io::Result<std::process::ExitStatus>) -> Result<(), String> {
    match status {
        Ok(s) if s.success() => Ok(()),
        Ok(s) => Err(s.to_string()),
        Err(e) => Err(e.to_string()),
    }
}

/// The process group of a run: the program and all it starts, ended as one when the run ends,
/// however it ends. Each is noted in its town while it lives, so the town reaches them all.
pub(crate) struct Group(Rc<Town>, unix::Pid);

impl Group {
    pub(crate) fn of(town: &Rc<Town>, child: &Child) -> Option<Group> {
        let id = unix::Pid::try_from(child.id()?).ok()?;
        town.groups.borrow_mut().insert(id);
        Some(Group(town.clone(), id))
    }
}

impl Drop for Group {
    fn drop(&mut self) {
        // one the town has ended already is not ended twice
        if self.0.groups.borrow_mut().remove(&self.1) {
            unix::end_group(self.1);
        }
    }
}

/// End the group of every program running in a town.
fn end_groups(town: &Town) {
    town.groups.borrow_mut().drain().for_each(unix::end_group);
}

/// Wait for the town to be stopped, end every program's group, since a signal to the town's own
/// group does not reach theirs, and end as a shell says that signal ended it: 128 and its number.
pub async fn ending(town: Rc<Town>) {
    use tokio::signal::unix::{signal, SignalKind};
    // a signal the town was started to ignore (`nohup`, a job in the background) it still ignores:
    // to listen for it would undo that
    let on = |kind: SignalKind| {
        let ignored = unix::ignored(kind.as_raw_value());
        (!ignored).then(|| signal(kind).expect("signals are heard"))
    };
    let heard = |s: Option<tokio::signal::unix::Signal>| async move {
        match s {
            Some(mut s) => _ = s.recv().await,
            None => std::future::pending().await,
        }
    };
    let (term, int, hup) = (SignalKind::terminate(), SignalKind::interrupt(), SignalKind::hangup());
    let sig = tokio::select! {
        _ = heard(on(term)) => term,
        _ = heard(on(int)) => int,
        _ = heard(on(hup)) => hup,
    };
    end_groups(&town);
    std::process::exit(128 + sig.as_raw_value()) // as a shell says a signal ended it
}

/// Tasks that end with whatever holds them, however it ends: a reopen's abort too.
struct Tasks(Vec<tokio::task::JoinHandle<()>>);

impl Drop for Tasks {
    fn drop(&mut self) {
        self.0.iter().for_each(|h| h.abort());
    }
}

/// The words a run is handed, a line each on its stdin. One that does not read them has, at most,
/// so many waiting; those after are dropped, and that is said once a run.
struct Hand {
    tx: mpsc::Sender<Word>,
    told: bool,
}

impl Hand {
    fn over(&mut self, town: &Town, from: &str, w: Word) {
        // closed, not full: it stopped reading for good, and how it ended is said
        if let Err(mpsc::error::TrySendError::Full(_)) = self.tx.try_send(w) {
            if !std::mem::replace(&mut self.told, true) {
                town.error(from, None, "is not reading: words dropped".into());
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
    use crate::testing::{guard, until, upto};
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

    // a watched file already kept in the present is heard once as the town opens, in the greeting;
    // a save that replaces the file, as `post` does, is still seen, each time
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
            // each save is heard once, and next: a second hearing of one would come before it
            let mut heard = vec![until(&mut words, |w| w.kind == "heard").await.body];
            for bg in ["bg=#111", "bg=#222", "bg=#333"] {
                std::fs::write(dir.join("colors.writing"), bg).unwrap();
                std::fs::rename(dir.join("colors.writing"), &file).unwrap();
                heard.push(until(&mut words, |w| w.kind == "heard").await.body);
            }
            assert_eq!(heard, ["bg=#000", "bg=#111", "bg=#222", "bg=#333"]);
            end(&town);
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
            let bg = |s: &str| Value::String(s.into());
            assert_eq!(until(&mut words, |_| true).await.body, bg("bg=#000"));
            std::fs::remove_file(&file).unwrap();
            assert_eq!(until(&mut words, |_| true).await.body, Value::Null);
            assert!(!town.present.borrow().contains_key("colors"), "a gone file is not present");
            std::fs::write(&file, "bg=#fff").unwrap();
            assert_eq!(until(&mut words, |_| true).await.body, bg("bg=#fff"));
            // an editor's save that takes the file away a moment: whether the gap is seen is the
            // filesystem's to say, but what the file holds after it always is
            std::fs::remove_file(&file).unwrap();
            std::fs::write(&file, "bg=#abc").unwrap();
            until(&mut words, |w| w.body == bg("bg=#abc")).await;
            end(&town);
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
            let said = until(&mut words, |_| true).await;
            assert_eq!(
                (said.kind.as_str(), &said.body),
                ("colors", &Value::String("bg=#000".into()))
            );
            end(&town);
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
        let kinds: Vec<String> =
            lua.load(r#"return react{ on("error", print), on({}, print) }.kinds"#).eval().unwrap();
        assert_eq!(kinds, ["*", "error"], "every kind, and failings, named");
    }

    // "*" is every kind but failings, which are named to be heard
    #[test]
    fn every_kind_is_all_but_failings() {
        assert!(hears("*", "note") && hears("error", "error") && hears("note", "note"));
        assert!(!hears("*", "error") && !hears("note", "error"));
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

    // a folder's own files come before its folders, so a library beside them is there first; a
    // linked folder is read like any other, once, so a link back up ends; a program, however deep
    #[test]
    fn a_folder_reads_its_files_then_its_folders() {
        let dir = std::env::temp_dir().join(format!("town-walk-{}", std::process::id()));
        let shared = dir.join("shared");
        std::fs::create_dir_all(dir.join("town/apps")).unwrap();
        std::fs::create_dir_all(&shared).unwrap();
        for f in ["town/lib.lua", "town/apps/echo.lua", "town/notes.txt", "shared/linked.lua"] {
            std::fs::write(dir.join(f), "").unwrap();
        }
        script(&dir.join("town"), "run", "");
        script(&dir.join("town"), "run~", ""); // an editor's backup
        script(&dir.join("town/apps"), "deep", "");
        std::os::unix::fs::symlink(&shared, dir.join("town/b-linked")).unwrap();
        std::os::unix::fs::symlink(dir.join("town"), dir.join("town/apps/up")).unwrap();
        std::fs::create_dir_all(dir.join("town/.git")).unwrap();
        std::fs::write(dir.join("town/.git/hidden.lua"), "").unwrap();
        std::fs::write(dir.join("town/.#lib.lua"), "").unwrap();
        let names: Vec<String> = files(&[dir.join("town")])
            .files
            .iter()
            .map(|p| p.strip_prefix(dir.join("town")).unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["lib.lua", "run", "apps/deep", "apps/echo.lua", "b-linked/linked.lua"]);
        // a file named by itself and in its folder runs once, where it first comes
        let twice = files(&[dir.join("town/apps/echo.lua"), dir.join("town")]).files;
        assert_eq!(twice.len(), 5);
        assert!(twice[0].ends_with("apps/echo.lua"));
        std::fs::remove_dir_all(&dir).ok();
    }

    /// a town's tasks ended, as a reopen ends them
    fn end(town: &Town) {
        town.tasks.borrow_mut().drain(..).for_each(|h| h.abort());
    }

    /// a shell script in `dir`, a program town may run
    fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    // a program runs once: one that fails says why, on stderr and in how it ended; one that ends
    // well says nothing; one that never says what it is, or ends before it does, says so. None runs
    // again. What it owns is its own only while it runs
    #[test]
    fn a_program_runs_once() {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        tokio::task::LocalSet::new().block_on(&rt, async {
            let dir = std::env::temp_dir().join(format!("town-ran-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            let began = r#"echo '{}'; echo '{"kind":"began","tense":"past"}'"#;
            let failing = script(&dir, "failing", &format!("{began}; echo oops >&2; exit 3"));
            let ending = r#"echo '{"kinds":["go"],"own":["mine"]}'
                echo '{"kind":"up","tense":"past"}'; read -r _"#;
            let ending = script(&dir, "ending", ending);
            let mute = script(&dir, "mute", r#"echo '{"kind":"began"}'"#);
            let early = script(&dir, "early", "exit 4");
            let town = Town::with_log(dir.join("past.log"));
            let mut words = town.words.subscribe();
            open(&Lua::new(), &town, &[failing, ending, mute, early]);
            let mut runs: Vec<_> = town.tasks.borrow_mut().drain(..).collect();
            // ending owns what it says it does while it runs, and gives it back as it ends
            let mine = Word::new("mine", Tense::Past, Value::Null);
            let mut said = upto(&mut words, |w| w.kind == "up").await;
            assert!(!town.admits(&mine), "its own, while it runs");
            town.talk(Word::new("go", Tense::Past, Value::Null));
            // every run ends, and its task with it: all they will ever say has been said
            for run in &mut runs {
                guard(run).await.unwrap();
            }
            assert!(town.admits(&mine), "given back");
            said.extend(std::iter::from_fn(|| words.try_recv().ok()));
            let from = |from: &str, is: &str| {
                said.iter()
                    .filter(|w| w.kind == "error" && w.body["from"] == from)
                    .filter(|w| w.body["message"].as_str().unwrap().starts_with(is))
                    .count()
            };
            assert_eq!(said.iter().filter(|w| w.kind == "began").count(), 1);
            assert_eq!(from("failing", ""), 2, "its stderr, and how it ended");
            assert_eq!(from("failing", "oops"), 1);
            assert_eq!(from("failing", "ended: exit status: 3"), 1);
            assert_eq!(from("ending", ""), 0);
            assert_eq!(from("mute", ""), 1);
            assert_eq!(from("mute", "its first line must say what it is"), 1);
            assert_eq!(from("early", ""), 1);
            assert_eq!(from("early", "ended before it said what it is: exit status: 4"), 1);
            runs.iter().for_each(|r| r.abort());
            std::fs::remove_dir_all(&dir).ok();
        });
    }

    // a program hears the present of its kinds, then each word of them, and nothing else; it
    // watches and owns as a Lua resident does, its paths beside it
    #[test]
    fn a_program_hears_its_kinds() {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        tokio::task::LocalSet::new().block_on(&rt, async {
            let dir = std::env::temp_dir().join(format!("town-hears-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("colors"), "bg=#000").unwrap();
            let parrot = script(
                &dir,
                "parrot",
                r#"echo '{"kinds":["colors","ping"],"watch":{"colors":"colors"},"own":["heard"]}'
                while read -r w; do
                  printf '{"kind":"heard","tense":"past","body":%s}\n' "$w"
                done"#,
            );
            let town = Town::with_log(dir.join("past.log"));
            town.talk(Word::new("theme", Tense::Present, Value::String("dark".into())));
            let mut words = town.words.subscribe();
            open(&Lua::new(), &town, &[parrot]);
            let heard = |w: &Word| w.kind == "heard";
            assert_eq!(
                until(&mut words, heard).await.body["kind"],
                "colors",
                "its watch, not the theme"
            );
            // a pong, then a ping: it hears in order, so a pong heard would come first
            town.talk(Word::new("pong", Tense::Past, Value::Null));
            town.talk(Word::new("ping", Tense::Past, Value::Null));
            assert_eq!(until(&mut words, heard).await.body["kind"], "ping");
            assert!(!town.admits(&Word::new("heard", Tense::Past, Value::Null)));
            assert!(!town.admits(&Word::new("colors", Tense::Present, Value::Null)));
            end(&town);
            std::fs::remove_dir_all(&dir).ok();
        });
    }

    // a town opens again when its programs change, however deep, and only then: not for an
    // editor's backup or lock, or what is no program
    #[test]
    fn a_town_opens_again_when_its_programs_change() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("town-look-{}", std::process::id()));
        let top = dir.join("town");
        std::fs::create_dir_all(top.join("target")).unwrap();
        std::fs::create_dir_all(top.join("apps")).unwrap();
        std::fs::write(top.join("apps/a.lua"), "").unwrap();
        let shout = script(&top, "shout.py", "");
        let ask = script(&top, "ask.sh", "");
        let given = [top.clone()];
        let mut was = look(&files(&given).files);
        let mut changed = || {
            let before = std::mem::replace(&mut was, look(&files(&given).files));
            before != was
        };
        std::fs::write(top.join("Cargo.toml"), "").unwrap();
        script(&top, "run~", "");
        std::os::unix::fs::symlink("x", top.join(".#lock")).unwrap();
        assert!(!changed(), "a stray file, an editor's backup and lock");
        script(&top.join("target"), "built", "");
        assert!(changed(), "a program, however deep");
        std::fs::write(top.join("apps/a.lua"), "-- saved").unwrap();
        assert!(changed(), "a program saved");
        std::fs::set_permissions(&ask, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(changed(), "a program no longer runnable");
        std::fs::remove_file(&shout).unwrap();
        assert!(changed(), "a program taken away, whatever it is named");
        std::fs::rename(top.join("apps"), dir.join("away")).unwrap();
        assert!(changed(), "a folder of programs, taken away");
        script(&top, "made", "");
        assert!(changed(), "a program made");
        std::fs::remove_dir_all(&dir).ok();
    }

    // a change that matters opens the town again: proven by a word only a program the change made
    // can say. The town is made aside and moved into place, so making it is no change it hears
    #[test]
    fn a_change_that_matters_opens_the_town_again() {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        tokio::task::LocalSet::new().block_on(&rt, async {
            let dir = std::env::temp_dir().join(format!("town-keep-{}", std::process::id()));
            let (top, aside) = (dir.join("town"), dir.join("aside"));
            let says = |kind: &str| format!(r#"echo '{{}}'; echo '{{"kind":"{kind}"}}'; exec cat"#);
            let made = |at: &Path, kind: &str| _ = script(at, kind, &says(kind));
            std::fs::create_dir_all(&aside).unwrap();
            made(&aside, "first");
            std::fs::rename(&aside, &top).unwrap();
            let town = Town::with_log(dir.join("past.log"));
            let lua = Lua::new();
            let mut words = town.words.subscribe();
            keep(&lua, &town, vec![top.clone()]);
            until(&mut words, |w| w.kind == "first").await;
            made(&top, "second");
            until(&mut words, |w| w.kind == "second").await;
            // the folder itself, replaced whole
            std::fs::create_dir_all(&aside).unwrap();
            made(&aside, "third");
            std::fs::remove_dir_all(&top).unwrap();
            std::fs::rename(&aside, &top).unwrap();
            until(&mut words, |w| w.kind == "third").await;
            // a program built beside the town, linked into it: built again, it runs again
            let beside = dir.join("beside");
            std::fs::create_dir_all(&beside).unwrap();
            let bot = script(&beside, "bot", &says("built"));
            std::os::unix::fs::symlink(&bot, top.join("bot")).unwrap();
            until(&mut words, |w| w.kind == "built").await;
            script(&beside, "bot", &says("rebuilt"));
            until(&mut words, |w| w.kind == "rebuilt").await;
            // linked before it is ever built, as after a clean: once built, it runs
            let release = dir.join("later/target/release");
            std::os::unix::fs::symlink(release.join("bot"), top.join("later")).unwrap();
            made(&top, "linked"); // heard once the town has seen the link, and watches for it
            until(&mut words, |w| w.kind == "linked").await;
            std::fs::create_dir_all(&release).unwrap();
            script(&release, "bot", &says("built-later"));
            until(&mut words, |w| w.kind == "built-later").await;
            end(&town);
            std::fs::remove_dir_all(&dir).ok();
        });
    }

    // a program never hears its own failings, so one that says again what it hears ends; and one
    // whose first line is no text is told it is no resident
    #[test]
    fn a_program_does_not_hear_itself_fail() {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        tokio::task::LocalSet::new().block_on(&rt, async {
            let dir = std::env::temp_dir().join(format!("town-itself-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            let loud = script(
                &dir,
                "loud",
                r#"echo '{"kinds":["*","error"]}'; while read -r w; do echo "$w" >&2; done"#,
            );
            let binary = script(&dir, "binary", r"printf '\377\n'; exec sleep 60");
            let town = Town::with_log(dir.join("past.log"));
            let mut words = town.words.subscribe();
            open(&Lua::new(), &town, &[loud, binary]);
            let failed =
                |from: &'static str| move |w: &Word| w.kind == "error" && w.body["from"] == from;
            let told = until(&mut words, failed("binary")).await;
            assert!(told.body["message"].as_str().unwrap().starts_with("its first line"));
            // loud hears the binary's failing, but not its own saying of it: the next it says is
            // pong
            until(&mut words, failed("loud")).await;
            town.talk(Word::new("pong", Tense::Past, Value::Null));
            let next = until(&mut words, failed("loud")).await;
            assert!(next.body["message"].as_str().unwrap().contains("pong"), "{next:?}");
            end(&town);
            std::fs::remove_dir_all(&dir).ok();
        });
    }

    // a program that does not read has only so many words waiting: what is dropped is said, once
    #[test]
    fn a_program_that_does_not_read_is_said_once() {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        tokio::task::LocalSet::new().block_on(&rt, async {
            let dir = std::env::temp_dir().join(format!("town-deaf-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            let deaf =
                r#"echo '{"kinds":["ping"]}'; echo '{"kind":"up","tense":"past"}'; exec sleep 60"#;
            let deaf = script(&dir, "deaf", deaf);
            let town = Town::with_log(dir.join("past.log"));
            let mut words = town.words.subscribe();
            open(&Lua::new(), &town, &[deaf]);
            until(&mut words, |w| w.kind == "up").await;
            let dropped = |w: &Word| w.body["message"] == "is not reading: words dropped";
            let (mut said, mut pings) = (0, 0);
            // pings until the pipe and the queue are full and it is said; then as many again
            while said == 0 || pings < 20_000 {
                town.talk(Word::new("ping", Tense::Past, Value::Null));
                pings += 1;
                // every word heard by all, so it falls behind no one
                while !town.words.is_empty() {
                    tokio::task::yield_now().await;
                    said += std::iter::from_fn(|| words.try_recv().ok()).filter(dropped).count();
                }
                assert!(pings < 100_000, "never said");
            }
            assert_eq!(said, 1);
            end(&town);
            std::fs::remove_dir_all(&dir).ok();
        });
    }

    // all a program starts ends with it: a child it left running, as a reopen ends the run
    #[test]
    fn a_programs_processes_end_with_it() {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        tokio::task::LocalSet::new().block_on(&rt, async {
            let dir = std::env::temp_dir().join(format!("town-group-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            let parent = r#"sleep 60 & echo $! > child
                echo '{}'; echo '{"kind":"up","tense":"past"}'; exec cat"#;
            let parent = script(&dir, "parent", parent);
            let town = Town::with_log(dir.join("past.log"));
            let mut words = town.words.subscribe();
            open(&Lua::new(), &town, &[parent]);
            until(&mut words, |w| w.kind == "up").await;
            let child: unix::Pid =
                std::fs::read_to_string(dir.join("child")).unwrap().trim().parse().unwrap();
            let alive = || unix::alive(child);
            assert!(alive(), "running while the program runs");
            end(&town);
            guard(async {
                while alive() {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await;
            std::fs::remove_dir_all(&dir).ok();
        });
    }
}
