//! The residents: what a town is. The README's *residents*.
//!
//! - **read**: from its paths, in order.
//! - **opened again**: at a future `town/open`, each saved alone, so it runs what is saved.
//! - **each**: a file that can run is a program (`program`), whatever it is written in; one that
//!   cannot, as a file of Lua, is a resident's to speak for, as `lua` speaks for each Lua file.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use tokio::time::Instant;

use crate::body::Body;
use crate::parts::Parts;
use crate::program::{residing, runs};
use crate::town::{Resident, Town, RESIDENTS};
use crate::word::{Tense, Word};

/// What a town opens first: its own residents, the `residents` folder in its crate, its languages
/// and `reload` among them; then `lua`, town's own program that speaks for each Lua file, as built
/// beside the one running, or above it, as a test runs in `deps/`, if it is built. Where nix
/// installs town, `share/town/residents` holds `lua` beside the rest, and is one path.
#[cfg(any(test, feature = "testing"))]
pub fn own() -> Vec<PathBuf> {
    let folder = Path::new(env!("CARGO_MANIFEST_DIR")).join("residents");
    let exe = std::env::current_exe().ok();
    let beside = exe.as_deref().and_then(Path::parent);
    let lua = beside.into_iter().flat_map(|at| [at.join("lua"), at.join("../lua")]);
    let lua = lua.map(|l| whole(&l)).find(|l| crate::program::runs(l));
    [folder].into_iter().chain(lua).collect()
}

/// Open a town at once, for a test: end what ran before, say its paths, whole, at
/// `town/residents`, and run its programs. Each, and the number of who it is.
#[cfg(any(test, feature = "testing"))]
pub fn open(town: &Rc<Town>, paths: &[PathBuf]) -> Vec<(PathBuf, u64)> {
    let paths: Vec<PathBuf> = paths.iter().map(|p| whole(p)).collect();
    town.reopen();
    say(town, &paths);
    all(town, &paths)
}

/// The whole town comes: each program of its paths runs, and what was held is held only for those
/// yet to come. Each, and the number of who it is.
fn all(town: &Rc<Town>, paths: &[PathBuf]) -> Vec<(PathBuf, u64)> {
    let ran = comes(town, &files(paths, runs));
    holding(town, &ran.iter().map(|(f, _)| f.clone()).collect::<Vec<_>>(), |_| true);
    ran
}

/// The paths, whole, said at `town/residents`, so a resident that reads them, as `lua` does, may.
fn say(town: &Town, paths: &[PathBuf]) {
    let said: Vec<Parts> = paths.iter().map(|p| p.to_string_lossy().as_ref().into()).collect();
    town.talk(Word::new(RESIDENTS, Tense::Present, Body::of(Parts::List(said))));
}

/// What was held of what one opening `saved` is kept only for those of the programs that `came`
/// yet to ask for it, until each says what it is, and for those one here speaks for, which it lets
/// go itself. The rest is let go, as all else that will come again has.
fn holding(town: &Town, came: &[PathBuf], saved: impl Fn(&str) -> bool) {
    let program = |kind: &str| came.iter().any(|p| under(kind, &fact(p)));
    town.hold_only(|kind| !saved(kind) || program(kind) || spoken(town, kind));
}

/// Whether the resident whose fact would be at `kind` is under one here, which speaks for it.
fn spoken(town: &Town, kind: &str) -> bool {
    let present = town.present.borrow();
    let mut at = kind;
    while let Some(up) = at.rfind('/').map(|i| &at[..i]) {
        if up.len() <= RESIDENTS.len() {
            return false;
        }
        if present.contains_key(up) {
            return true;
        }
        at = up;
    }
    false
}

/// Each program of `files` runs, in order. Each, and the number of who it is.
/// Each is by its file's name, at its whole path.
fn comes(town: &Rc<Town>, files: &[PathBuf]) -> Vec<(PathBuf, u64)> {
    let mut ran = Vec::new();
    for path in files {
        let name = path.file_name().map_or("?".into(), |n| n.to_string_lossy().into_owned());
        let placed = whole(path).to_string_lossy().trim_start_matches('/').to_string();
        let it = Resident { placed: Some(placed), ..town.who(&name, false) };
        ran.push((path.clone(), it.id));
        town.runs(it.id, tokio::task::spawn_local(residing(town.clone(), it, path.clone())));
    }
    ran
}

/// Where the fact of the resident a program is would be.
fn fact(path: &Path) -> String {
    format!("{RESIDENTS}/{}", whole(path).to_string_lossy().trim_start_matches('/'))
}

/// Whether `kind` is `at`, or under it.
pub(crate) fn under(kind: &str, at: &str) -> bool {
    kind.strip_prefix(at).is_some_and(|r| r.is_empty() || r.starts_with('/'))
}

/// Whether a file is one a walk finds: never a hidden file (`.git`, an editor's `.#lock`), nor an
/// editor's backup (`name~`).
fn shown(path: &Path) -> bool {
    let name = path.file_name().map(|n| n.to_string_lossy()).unwrap_or_default();
    !name.starts_with('.') && !name.ends_with('~')
}

/// The files of a town that are `is`, in the order they open: each path as given; a folder's, as a
/// `Walk` finds them, its own files before its folders, none hidden; each folder read once, though
/// two paths hold it. A file named twice comes once, where it first comes. Town's are its
/// programs (`program::runs`), wherever they are in its folders, so a build is kept outside them
/// and its program linked in; a speaker's, those it runs.
pub fn files(paths: &[PathBuf], is: impl Fn(&Path) -> bool) -> Vec<PathBuf> {
    let (mut walk, mut files) = (Walk::default(), Vec::new());
    for p in paths {
        if p.is_dir() {
            walk.read(p, p, Path::new(""));
            files.extend(walk.files.drain(..).map(|(f, ..)| f).filter(|f| shown(f) && is(f)));
        } else if is(p) {
            files.push(p.clone());
        }
    }
    let mut ran = HashSet::new();
    files.retain(|f| ran.insert(f.canonicalize().unwrap_or(f.clone())));
    files
}

/// Open a town, and keep it to open again: say its paths, whole, at `town/residents`, and open.
/// Its `Opening` answers a future `town/open` of a program (`work::begin`), asked by a resident
/// that watches, as `reload` does.
pub fn keep(town: &Rc<Town>, paths: Vec<PathBuf>) {
    let paths: Vec<PathBuf> = paths.iter().map(|p| whole(p)).collect();
    say(town, &paths);
    let opening = Rc::new(Opening {
        paths,
        ran: Default::default(),
        asked: Default::default(),
        settling: Default::default(),
    });
    *town.opening.borrow_mut() = Some(opening.clone());
    opening.open(town);
}

/// What a town keeps to open again: its paths, each program it ran and the number of who it is,
/// the files one save touched, and, while its opens come, when the last came and the most it
/// waits. Town's own, so it outlives every opening.
pub struct Opening {
    paths: Vec<PathBuf>,
    ran: RefCell<Vec<(PathBuf, u64)>>,
    asked: RefCell<Vec<PathBuf>>,
    settling: Cell<Option<(Instant, Instant)>>,
}

impl Opening {
    /// A future `town/open`: a save, of any file of the town. Its opens come a few ms apart, so it
    /// is settled once none has come for `SETTLE`, or a second after the first, if they never stop;
    /// every open meanwhile is of that save.
    pub(crate) fn ask(self: &Rc<Self>, town: &Rc<Town>, body: &Body) {
        let Some(path) = matters(body, &self.paths, &self.ran.borrow()) else { return };
        let mut asked = self.asked.borrow_mut();
        if !asked.contains(&path) {
            asked.push(path);
        }
        drop(asked);
        let now = Instant::now();
        if let Some((_, most)) = self.settling.get() {
            self.settling.set(Some((now, most)));
            return;
        }
        self.settling.set(Some((now, now + Duration::from_secs(1))));
        // town's own, so it outlives a reopen; holding the town weakly, so a town done is freed
        let (me, town) = (self.clone(), Rc::downgrade(town));
        tokio::task::spawn_local(async move {
            while let Some((last, most)) = me.settling.get() {
                let at = (last + SETTLE).min(most);
                if Instant::now() >= at {
                    break;
                }
                tokio::time::sleep_until(at).await;
            }
            me.settling.set(None);
            let asked = me.asked.take();
            if let Some(town) = town.upgrade() {
                me.again(&town, &asked);
            }
        });
    }

    /// One save, settled: each program saved goes and comes again, alone, and one deleted goes;
    /// then the save is said, a past `town/open` of every file it touched, so one that speaks for
    /// files opens those it runs.
    fn again(&self, town: &Rc<Town>, asked: &[PathBuf]) {
        let ran = self.ran.borrow().clone();
        for path in asked {
            if let Some((_, id)) = ran.iter().find(|(f, _)| f == path) {
                town.leaves(&fact(path), *id);
            }
        }
        // what is there: one deleted only goes
        let coming: Vec<PathBuf> =
            asked.iter().filter(|p| shown(p) && runs(p) && p.exists()).cloned().collect();
        let came = comes(town, &coming);
        holding(town, &coming, |kind| asked.iter().any(|p| under(kind, &fact(p))));
        let mut ran = self.ran.borrow_mut();
        ran.retain(|(f, _)| !asked.contains(f));
        ran.extend(came);
        drop(ran);
        let paths = asked.iter().map(|p| p.to_string_lossy().as_ref().into()).collect();
        let saved = Parts::map([("paths", Parts::List(paths))]);
        town.talk(Word::new("town/open", Tense::Past, Body::of(saved)));
    }

    /// The whole town opens again: all that ran ends; then each program runs, and hears the paths,
    /// said before it, in its greeting. What it ran, kept.
    fn open(&self, town: &Rc<Town>) {
        town.reopen();
        *self.ran.borrow_mut() = all(town, &self.paths);
    }
}

/// A path as one place, read as a path is: `/a/b` for `/a/./b/`, `/a/c/../b` or `a/b` from `/`,
/// so each part of it names something, as a kind's must. A link in it is left as it is.
pub fn whole(path: &Path) -> PathBuf {
    use std::path::Component;
    let path = std::path::absolute(path).unwrap_or(path.to_path_buf());
    let mut whole = PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => drop(whole.pop()),
            part => whole.push(part),
        }
    }
    whole
}

/// The file a `town/open` asks to open, whole, if it matters: one of the town's, at or in one of
/// its `paths` and nothing hidden on the way, as a walk finds them; or a program it `ran`.
fn matters(body: &Body, paths: &[PathBuf], ran: &[(PathBuf, u64)]) -> Option<PathBuf> {
    let path = whole(Path::new(body.part("path").and_then(Parts::text)?));
    let within = |at: &PathBuf| {
        let inside = path.strip_prefix(at).ok();
        inside.is_some_and(|i| i.components().all(|c| shown(Path::new(c.as_os_str()))))
    };
    let mine = shown(&path) && paths.iter().any(within);
    (mine || ran.iter().any(|(f, _)| *f == path)).then_some(path)
}

/// How quiet a save must fall before what it saved opens again.
const SETTLE: Duration = Duration::from_millis(50);

/// A walk of folders, as a town reads its residents and a watch what it holds: each file, however
/// deep, a folder's own files in name order, then its folders; nothing hidden. A link is followed,
/// but not into the walk itself, so each file is found once, where it truly is, and each folder is
/// read once.
#[derive(Default)]
pub struct Walk {
    /// each file found: where it is, as it was reached; its name in the walk; what it is
    pub files: Vec<(PathBuf, PathBuf, std::fs::Metadata)>,
    /// each folder read, as it truly is
    pub folders: HashSet<PathBuf>,
    /// where each link followed leads, by its name in the walk: a folder, whole, `true`; a file,
    /// by its folder; one that leads nowhere yet, the nearest folder there is
    pub leads: BTreeMap<String, (PathBuf, bool)>,
}

impl Walk {
    /// Read `path`, named `name` in the walk of `root`, and all in it.
    pub fn read(&mut self, root: &Path, path: &Path, name: &Path) {
        let Ok(meta) = std::fs::symlink_metadata(path) else { return };
        let key = || name.to_string_lossy().into_owned();
        let meta = match meta.is_symlink() {
            false => meta,
            true => match (path.canonicalize(), std::fs::metadata(path)) {
                (Ok(to), Ok(meta)) => {
                    let inside = root.canonicalize().is_ok_and(|r| to.starts_with(r));
                    if !name.as_os_str().is_empty() && inside {
                        return; // found where it truly is
                    }
                    if meta.is_dir() && self.folders.contains(&to) {
                        return; // read already, where it was first found
                    }
                    let at = match meta.is_dir() {
                        true => to,
                        false => to.parent().map_or(to.clone(), Path::to_path_buf),
                    };
                    self.leads.insert(key(), (at, meta.is_dir()));
                    meta
                }
                // it leads nowhere yet: as near as there is
                _ => {
                    if let Some(at) = nearest(path) {
                        self.leads.insert(key(), (at, false));
                    }
                    return;
                }
            },
        };
        if !meta.is_dir() {
            self.files.push((path.to_path_buf(), name.to_path_buf(), meta));
            return;
        }
        if !self.folders.insert(path.canonicalize().unwrap_or(path.to_path_buf())) {
            return;
        }
        let entries = std::fs::read_dir(path).into_iter().flatten().filter_map(Result::ok);
        let mut shown: Vec<_> =
            entries.filter(|e| !e.file_name().to_string_lossy().starts_with('.')).collect();
        shown.sort_by_key(|e| e.file_name());
        let (folders, files): (Vec<_>, Vec<_>) = shown.iter().partition(|e| e.path().is_dir());
        for e in files.into_iter().chain(folders) {
            self.read(root, &e.path(), &name.join(e.file_name()));
        }
    }
}

/// For a link that leads nowhere yet, the nearest folder there is on its way.
fn nearest(link: &Path) -> Option<PathBuf> {
    let to = link.parent()?.join(std::fs::read_link(link).ok()?);
    to.ancestors().skip(1).find_map(|a| a.canonicalize().ok())
}

#[cfg(test)]
pub(crate) mod inside {
    use super::*;
    use crate::program::LAST;
    use crate::testing::{an_error, quiet, run, scratch, until, Scratch};
    use crate::town::Listening;

    /// `lua`, town's own program for its Lua files, as built: `cargo test` builds it.
    pub(crate) fn lua() -> PathBuf {
        let lua = own().into_iter().find(|p| p.is_file());
        lua.expect("lua is built beside the tests: cargo build --bin lua")
    }

    /// Where the fact of a Lua file `lua` speaks for is: under `lua`'s place, by its whole path.
    pub(crate) fn lua_fact(path: &Path) -> String {
        format!("{}/{}", fact(&lua()), whole(path).to_string_lossy().trim_start_matches('/'))
    }

    /// End what runs for a town by its tasks alone, not as a reopen does, so what lets go as a
    /// task ends is seen to.
    pub(crate) fn end(town: &Town) {
        town.tasks.borrow_mut().drain(..).for_each(|(_, h)| h.abort());
    }

    /// `lua`, run once, as a town opens it, speaking for the Lua file at `path` beside those it
    /// speaks for already, the paths at `town/residents`: here once the file has come. Named
    /// again, it comes again, as it is now.
    pub(crate) async fn wired(town: &Rc<Town>, path: &Path) {
        let path = whole(path);
        let mut words = town.words.listen();
        let said = town.facts(RESIDENTS).remove(RESIDENTS);
        let said = said.as_ref().and_then(|b| b.parts().list()).into_iter().flatten();
        let mut paths: Vec<PathBuf> = said.filter_map(Parts::text).map(PathBuf::from).collect();
        let runs = town.present.borrow().contains_key(&fact(&lua()));
        if !paths.contains(&path) {
            paths.push(path.clone());
            say(town, &paths);
        } else if runs {
            town.talk(Word::new("town/open", Tense::Future, asked(&path)));
        }
        if !runs {
            comes(town, &[lua()]);
        }
        let at = format!("/{}", path.to_string_lossy().trim_start_matches('/'));
        let came = |w: &Word| w.kind.starts_with(RESIDENTS) && w.kind.ends_with(&at);
        until(&mut words, |w| came(w) && !w.body.is_none()).await;
    }

    /// a resident that is the language `name`, its grammar in Lua: a file in `dir`, `wired`
    pub(crate) async fn speaking(town: &Rc<Town>, dir: &Path, name: &str, grammar: &str) {
        let path = dir.join(format!("{name}.lua"));
        std::fs::write(&path, format!("return {{ languages = {{ {name} = {grammar} }} }}"))
            .unwrap();
        wired(town, &path).await;
    }

    /// a shell script in `dir`, a program town may run
    pub(crate) fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    // a folder's own files come before its folders; a program, however deep, whatever it is
    // written in; a linked folder read like any other, once, so a link back up ends; no hidden file
    // or folder, no backup, nothing that cannot run, as a Lua file that cannot
    #[test]
    fn a_folder_reads_its_files_then_its_folders() {
        let dir = scratch();
        let (top, shared) = (dir.join("town"), dir.join("shared"));
        std::fs::create_dir_all(top.join("apps")).unwrap();
        std::fs::create_dir_all(top.join(".git")).unwrap();
        std::fs::create_dir_all(&shared).unwrap();
        for f in ["lib.lua", "apps/echo.lua", "notes.txt"] {
            std::fs::write(top.join(f), "").unwrap();
        }
        script(&top, "run", "");
        script(&top, "run~", ""); // an editor's backup
        script(&top, ".#run", ""); // an editor's lock
        script(&top.join(".git"), "hidden", "");
        script(&top.join("apps"), "deep", "");
        script(&top.join("apps"), "runs.lua", ""); // a Lua file that runs itself
        script(&shared, "linked", "");
        std::os::unix::fs::symlink(&shared, top.join("b-linked")).unwrap();
        std::os::unix::fs::symlink(&top, top.join("apps/up")).unwrap();
        let names = |files: Vec<PathBuf>| -> Vec<String> {
            let names = files.iter().map(|p| p.strip_prefix(&top).unwrap());
            names.map(|p| p.to_string_lossy().into_owned()).collect()
        };
        let all = ["run", "apps/deep", "apps/runs.lua", "b-linked/linked"];
        assert_eq!(names(files(std::slice::from_ref(&top), runs)), all);
        // a file named by itself and in its folder comes once, where it first comes
        let twice = files(&[top.join("apps/deep"), top.clone()], runs);
        assert_eq!(names(twice), ["apps/deep", "run", "apps/runs.lua", "b-linked/linked"]);
        assert!(files(&[top.join("lib.lua")], runs).is_empty(), "a file named that cannot run");
        assert!(files(&[top.join("none")], runs).is_empty(), "nor one not there");
    }

    // an open matters to any file of the town, however deep, gone or not: not to an editor's
    // backup or lock, one in a hidden folder, or one outside the town; a program the town ran
    // matters, wherever it is; and an open names a path
    #[test]
    fn only_an_open_of_a_towns_file_matters() {
        let dir = scratch();
        let top = dir.join("town");
        std::fs::create_dir_all(top.join("apps")).unwrap();
        let away = script(&dir, "away", "");
        let (paths, ran) = (vec![whole(&top)], vec![(whole(&away), 0)]);
        let changed = |path: &Path| matters(&asked(path), &paths, &ran).is_some();
        assert!(changed(&top.join("apps/a.lua")), "a file of the town, however deep");
        assert!(changed(&top.join("gone")), "gone or not");
        assert!(!changed(&top.join("run~")) && !changed(&top.join(".#lock")), "a backup, a lock");
        assert!(!changed(&top.join(".git/config")), "one in a hidden folder");
        assert!(!changed(&dir.join("elsewhere")), "one outside the town");
        assert!(changed(&away), "a program it ran, wherever it is");
        assert!(matters(&Body::none(), &paths, &ran).is_none(), "an open names a path");
    }

    // a path is one place, read as a path is, so each part of it names something
    #[test]
    fn a_path_is_made_whole() {
        assert_eq!(whole(Path::new("/a/./b/")), Path::new("/a/b"));
        assert_eq!(whole(Path::new("/a/c/../b")), Path::new("/a/b"));
        assert_eq!(whole(Path::new("b")), std::env::current_dir().unwrap().join("b"));
    }

    // town's own: its folder, then `lua`, built, beside it; never the folder of what is built
    #[test]
    fn a_town_has_its_own() {
        let own = own();
        assert_eq!(own[0], Path::new(env!("CARGO_MANIFEST_DIR")).join("residents"));
        assert_eq!(own.len(), 2, "lua, built: {own:?}");
        assert!(own[1].ends_with("lua") && crate::program::runs(&own[1]), "{own:?}");
        assert!(files(&own[..1], runs).is_empty(), "the folder holds no program: {own:?}");
    }

    /// an open of `path`, as a resident asks it
    fn asked(path: &Path) -> Body {
        Body::of(Parts::map([("path", path.to_string_lossy().as_ref())]))
    }

    /// a Lua file that says it opened, as `lua` reads it, so each opening is heard, however soon
    /// the next; and a town kept from `lua` and `top`, heard from before it opens
    async fn kept(dir: &Scratch, top: &Path) -> (Rc<Town>, Listening) {
        std::fs::create_dir_all(top).unwrap();
        std::fs::write(top.join("opens.lua"), r#"talk(event("opened"))"#).unwrap();
        let town = Town::with_log(dir.log());
        let mut words = town.words.listen();
        keep(&town, vec![lua(), top.join(".")]);
        until(&mut words, |w| w.kind == "opened").await;
        (town, words)
    }

    /// whether the town's Lua opens again within a while
    async fn opens(words: &mut Listening) -> bool {
        let again = until(words, |w| w.kind == "opened");
        tokio::time::timeout(Duration::from_millis(300), again).await.is_ok()
    }

    // a town says the paths it is opened with, whole, at `town/residents`, and opens once
    #[test]
    fn a_town_opens_once_as_it_starts() {
        run(async {
            let dir = scratch();
            let top = dir.join("town");
            let (town, mut words) = kept(&dir, &top).await;
            assert!(!opens(&mut words).await, "opened once");
            let paths = town.facts(RESIDENTS).remove(RESIDENTS).unwrap();
            assert_eq!(paths.value(), serde_json::json!([lua(), top]), "the paths, whole");
            town.reopen();
        });
    }

    // a town done is freed: its programs and their joins do not hold it, so once what runs for it
    // ends and it is dropped, it is gone
    #[test]
    fn a_town_done_is_freed() {
        run(async {
            let dir = scratch();
            let top = dir.join("town");
            std::fs::create_dir_all(&top).unwrap();
            let joins = r#"talk(join("cat", { "cat" }))
                return react { own = { "mine" }, on("go", function() return event("mine") end) }"#;
            std::fs::write(top.join("joins.lua"), joins).unwrap();
            let town = Town::with_log(dir.log());
            let mut words = town.words.listen();
            keep(&town, vec![lua(), top.clone()]);
            let came = lua_fact(&top.join("joins.lua"));
            until(&mut words, |w| w.kind == came && !w.body.is_none()).await;
            town.talk(Word::new("go", Tense::Past, Body::none()));
            until(&mut words, |w| w.kind == "mine").await;
            if !town.joins.borrow().contains_key("cat") {
                until(&mut words, |w| w.kind == format!("{came}/cat")).await;
            }
            assert!(town.joins.borrow().contains_key("cat"), "it has a join");
            let gone = Rc::downgrade(&town);
            end(&town);
            drop((town, words));
            for _ in 0..16 {
                tokio::task::yield_now().await;
            }
            assert!(gone.upgrade().is_none(), "freed");
        });
    }

    // what a program owned is held from the square as the town opens again, though it says what
    // it is only once it runs; what a Lua file that is gone owned is let go as `lua` says all it
    // speaks for is said
    #[test]
    fn what_a_program_owned_is_held_as_the_town_opens_again() {
        run(async {
            let dir = scratch();
            let themer = r#"sleep 0.1; echo '{"own":["theme"]}'; exec sleep 60"#;
            let themer = script(&dir, "themer", themer);
            let gone = dir.join("gone.lua");
            std::fs::write(&gone, r#"return { own = { "gone" } }"#).unwrap();
            let town = Town::with_log(dir.log());
            let mut words = town.words.listen();
            open(&town, &[lua(), themer.clone(), gone.clone()]);
            until(&mut words, |w| w.kind == lua_fact(&gone) && !w.body.is_none()).await;
            let came = |w: &Word| w.kind.ends_with("/themer") && !w.body.is_none();
            until(&mut words, came).await;
            std::fs::remove_file(&gone).unwrap();
            open(&town, &[lua(), themer]);
            let owns = |own: &str| Resident { own: vec![own.into()], ..town.who("square", true) };
            assert!(town.come(&owns("theme")).is_err(), "held, before it says what it is");
            assert!(town.come(&owns("gone")).is_err(), "held, before lua says what it is");
            // let go as lua says all it speaks for is said, once it has opened its files
            let free = async {
                while town.come(&owns("gone")).is_err() {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            };
            let free = tokio::time::timeout(Duration::from_secs(5), free).await;
            assert!(free.is_ok(), "a Lua file gone: let go");
            until(&mut words, came).await;
            assert!(town.come(&owns("theme")).is_err(), "its own again");
            town.reopen();
        });
    }

    // a future `town/open` of a resident opens it again, alone, and the save is said; of a Lua
    // library, which the Lua files after it are written with, every Lua file, as `lua` opens them,
    // and no program; of what is no resident, nothing; of a resident deleted, it goes; and many
    // opens of one save open once
    #[test]
    fn an_open_of_a_resident_opens_it_again() {
        run(async {
            let dir = scratch();
            let top = dir.join("town");
            let (town, mut words) = kept(&dir, &top).await;
            let open = |path: &Path| town.talk(Word::new("town/open", Tense::Future, asked(path)));
            std::fs::write(top.join("README.md"), "").unwrap();
            open(&top.join("README.md"));
            assert!(!opens(&mut words).await, "no resident");
            let new = r#"echo '{"own":["mine"]}'; echo '{"kind":"new"}'; exec cat"#;
            let new = script(&top, "new", new);
            open(&new);
            let said = until(&mut words, |w| w.kind == "town/open" && w.tense == Tense::Past);
            assert_eq!(
                said.await.body.value()["paths"],
                serde_json::json!([new]),
                "the save, said"
            );
            until(&mut words, |w| w.kind == "new").await;
            assert!(!opens(&mut words).await, "a new file comes, alone");
            let id = |kind: &str| town.facts(kind).remove(kind).map(|b| b.value()["id"].clone());
            let was = id(&fact(&new));
            (0..5).for_each(|_| open(&top.join("opens.lua")));
            assert!(opens(&mut words).await && !opens(&mut words).await, "once, for one save");
            assert!(was.is_some() && id(&fact(&new)) == was, "a program stays as it is");
            std::fs::remove_file(&new).unwrap();
            open(&new);
            until(&mut words, |w| w.kind == fact(&new) && w.body.is_none()).await;
            assert!(!opens(&mut words).await, "a resident deleted goes, alone");
            town.reopen();
        });
    }

    // a resident saved goes and comes again, and what it asked for with it: its join, under its
    // place, spoken for by it; the others stay as they are, by the same number
    #[test]
    fn a_save_opens_only_the_resident_saved() {
        run(async {
            let dir = scratch();
            let top = dir.join("town");
            std::fs::create_dir_all(&top).unwrap();
            let (a, b) = (top.join("a.lua"), top.join("b.lua"));
            std::fs::write(&a, r#"talk(join("cat", { "cat" })); return { own = { "a" } }"#)
                .unwrap();
            std::fs::write(&b, r#"return { own = { "b" } }"#).unwrap();
            let (town, mut words) = kept(&dir, &top).await;
            let open = |path: &Path| town.talk(Word::new("town/open", Tense::Future, asked(path)));
            let id = |kind: &str| town.facts(kind).remove(kind).map(|b| b.value()["id"].clone());
            let cat = format!("{}/cat", lua_fact(&a));
            let joined = |w: &Word| w.kind == cat && !w.body.is_none();
            if id(&cat).is_none() {
                until(&mut words, joined).await;
            }
            let (was, its, others) = (id(&lua_fact(&a)), id(&cat), id(&lua_fact(&b)));
            assert!(town.joins.borrow().contains_key("cat"), "joined, under the one that asked");

            open(&a);
            until(&mut words, |w| w.kind == cat && w.body.is_none()).await;
            until(&mut words, joined).await;
            assert!(id(&lua_fact(&a)) != was && id(&cat) != its, "it, and its join, come again");
            assert_eq!(id(&lua_fact(&b)), others, "the others stay");
            assert!(!opens(&mut words).await, "no more opens");

            std::fs::remove_file(&a).unwrap();
            open(&a);
            until(&mut words, |w| w.kind == lua_fact(&a) && w.body.is_none()).await;
            if id(&cat).is_some() {
                until(&mut words, |w| w.kind == cat && w.body.is_none()).await;
            }
            assert!(
                id(&cat).is_none() && !town.joins.borrow().contains_key("cat"),
                "its join, gone"
            );
            town.reopen();
        });
    }

    // a program with a fact is heard to its last line, though it goes as it ends: its going ends
    // what runs for it, but not the run that goes, which hears what is left of its stderr
    #[test]
    fn a_program_that_goes_is_heard_to_its_end() {
        run(async {
            let dir = scratch();
            // a job of its own group, which outlives it, says a line once it has gone
            let late = r#"echo '{"own":["late"]}'; set -m
                (sleep 0.2; echo late >&2) >/dev/null & exit 3"#;
            let late = script(&dir, "late", late);
            let town = Town::with_log(dir.log());
            let mut words = town.words.listen();
            open(&town, std::slice::from_ref(&late));
            until(&mut words, |w| w.kind == fact(&late) && w.body.is_none()).await;
            let heard = until(&mut words, an_error("late"));
            assert!(tokio::time::timeout(LAST, heard).await.is_ok(), "its last line");
            town.reopen();
        });
    }

    // a resident goes as one fact of nothing at each place it and those it spoke for held, and
    // none where none was: a program gone before it said what it is held none
    #[test]
    fn going_says_nothing_once_where_a_fact_was() {
        run(async {
            let dir = scratch();
            let joins = dir.join("joins.lua");
            std::fs::write(&joins, r#"talk(join("cat", { "cat" })); return { own = { "a" } }"#)
                .unwrap();
            let mute = script(&dir, "mute", "exec sleep 60");
            let town = Town::with_log(dir.log());
            let mut words = town.words.listen();
            let ran = open(&town, &[lua(), joins.clone(), mute.clone()]);
            let cat = format!("{}/cat", lua_fact(&joins));
            until(&mut words, |w| w.kind == cat && !w.body.is_none()).await;
            for (path, id) in &ran {
                town.leaves(&fact(path), *id);
            }
            let said = quiet(&mut words).await;
            let nothing =
                |at: &str| said.iter().filter(|w| w.kind == at && w.body.is_none()).count();
            let once = (nothing(&fact(&lua())), nothing(&lua_fact(&joins)), nothing(&cat));
            assert_eq!(once, (1, 1, 1), "once each");
            assert_eq!(nothing(&fact(&mute)), 0, "none where none was");
            town.reopen();
        });
    }

    // what a program held is held until it says what it is again, and what was under it until it
    // says all it speaks for is said, and let go then, so what it no longer asks for is not held
    // from the square for good: saved, or as the whole town opens again
    #[test]
    fn what_a_program_no_longer_asks_for_is_let_go() {
        run(async {
            let dir = scratch();
            let top = dir.join("town");
            std::fs::create_dir_all(&top).unwrap();
            let was = r#"echo '{"own":["p"]}'
                echo '{"kind":"town/join","tense":"future","body":{"as":"cat","argv":["cat"]}}'
                echo '{"kind":"town/residents/sub","body":{"own":["subs"]}}'
                exec sleep 60"#;
            let is = r#"echo '{"own":["p"]}'
                echo '{"kind":"town/residents","tense":"past"}'
                echo '{"kind":"up","tense":"past"}'
                exec sleep 60"#;
            let p = script(&top, "p", was);
            let town = Town::with_log(dir.log());
            let mut words = town.words.listen();
            keep(&town, vec![top.clone()]);
            let open = |path: &Path| town.talk(Word::new("town/open", Tense::Future, asked(path)));
            let owns = |own: &str| Resident { own: vec![own.into()], ..town.who("square", true) };
            let (cat, sub) = (format!("{}/cat", fact(&p)), format!("{}/sub", fact(&p)));
            for whole in [false, true] {
                script(&top, "p", was);
                if whole {
                    open(&p);
                }
                let mut asked = [false, false];
                while asked != [true, true] {
                    let w = until(&mut words, |w| !w.body.is_none()).await;
                    asked = [asked[0] || w.kind == cat, asked[1] || w.kind == sub];
                }
                script(&top, "p", is);
                match whole {
                    true => town.opening.borrow().clone().unwrap().open(&town),
                    false => open(&p),
                }
                until(&mut words, |w| w.kind == "up").await;
                assert!(town.come(&owns("p")).is_err(), "its own, again");
                assert!(town.come(&owns("cat")).is_ok(), "its join, let go");
                assert!(town.come(&owns("subs")).is_ok(), "one it spoke for, let go");
            }
            town.reopen();
        });
    }
}
