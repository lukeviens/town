//! The residents: a town's programs, files that listen to words and talk words back. A town is
//! read from the paths it is given, in order, and opened again whenever one changes, so it runs
//! what is saved. A Lua file is a Lua resident (`crate::lua`); a file town runs, in any language,
//! is a program (`program`); either may watch files (`watch`).

mod program;
mod watch;

pub use program::ending;
pub(crate) use program::Group;
pub(crate) use watch::{watch_for, Watched};

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use mlua::Lua;
use notify::{RecursiveMode, Watcher};
use tokio::sync::mpsc;

use crate::lua::wire;
use crate::town::Town;
use program::{end_groups, residing};

/// Read a town's programs, in the order given, and bring them to life: each path a program, or a
/// folder of them: its own files in name order, then its folders. The town grows older by one, so
/// what listened for it before stops; called once at boot and again whenever a program changes.
/// What failed is said last, once every new resident listens, so one may hear it.
pub fn open(lua: &Lua, town: &Rc<Town>, programs: &[PathBuf]) {
    open_read(lua, town, &files(programs).files);
}

/// `open`, with the programs already read.
fn open_read(lua: &Lua, town: &Rc<Town>, files: &[PathBuf]) {
    town.age.set(town.age.get() + 1);
    let age = town.age.get();
    // end what ran for the town before, its watches, joins and programs: they cannot reliably
    // notice a reopen themselves
    for h in town.tasks.borrow_mut().drain(..) {
        h.abort();
    }
    // and end their programs now, not as the tasks are dropped: a new run must not meet the old
    end_groups(town);
    town.joins.borrow_mut().clear();
    *town.inside.borrow_mut() = Town::machinery();
    town.languages.clear();

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
pub(crate) fn named(path: &Path) -> String {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or("?".into());
    name.strip_suffix(".lua").map(String::from).unwrap_or(name)
}

/// How quiet a save must fall before the town opens again: its events come a few ms apart.
const SETTLE: Duration = Duration::from_millis(50);

/// Whether a resident that hears `kind` hears a word of kind `said`. "*" is every kind but
/// `error`, which is named to be heard: two that hear everything, and fail at what they hear,
/// would otherwise answer each other's failings forever.
pub(crate) fn hears(kind: &str, said: &str) -> bool {
    kind == said || (kind == "*" && said != "error")
}

/// Tasks that end with whatever holds them, however it ends: a reopen's abort too.
pub(crate) struct Tasks(Vec<tokio::task::JoinHandle<()>>);

impl Drop for Tasks {
    fn drop(&mut self) {
        self.0.iter().for_each(|h| h.abort());
    }
}

#[cfg(test)]
pub(crate) mod inside {
    use super::*;
    use crate::testing::until;
    use crate::word::Tense;

    use crate::body::Body;
    use crate::word::Word;
    use mlua::Lua;

    pub(crate) fn word(kind: &str) -> Word {
        Word::new(kind, Tense::Future, Body::none())
    }

    /// a town's tasks ended, as a reopen ends them
    pub(crate) fn end(town: &Town) {
        town.tasks.borrow_mut().drain(..).for_each(|h| h.abort());
    }

    /// a shell script in `dir`, a program town may run
    pub(crate) fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    // "*" is every kind but failings, which are named to be heard
    #[test]
    fn every_kind_is_all_but_failings() {
        assert!(hears("*", "note") && hears("error", "error") && hears("note", "note"));
        assert!(!hears("*", "error") && !hears("note", "error"));
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
}
