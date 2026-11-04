//! The residents: what a town is. The README's *residents*.
//!
//! - **read**: from its paths, in order.
//! - **opened again**: at a future `town/open`, each saved alone, so it runs what is saved.
//! - **each**: a Lua file is a Lua resident (`crate::lua`); any other file town may run is a
//!   program (`program`).

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use mlua::Lua;
use tokio::time::Instant;

use crate::body::Body;
use crate::lua::wire;
use crate::parts::Parts;
use crate::program::residing;
use crate::town::{Resident, Town, RESIDENTS};
use crate::word::{Tense, Word};

/// Town's own residents, the `residents` folder in its crate: its languages, and `reload`. Where
/// nix installs town, it is given them in `share/town/residents` instead.
#[cfg(any(test, feature = "testing"))]
pub fn own() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("residents")
}

/// Open a town: end what ran before, and bring its residents to life, as `files` reads them. What
/// failed is said last, once every new resident listens, so one may hear it. Each file read, and
/// the number of who it is, if it came.
pub fn open(lua: &Lua, town: &Rc<Town>, paths: &[PathBuf]) -> Vec<(PathBuf, Option<u64>)> {
    // end what ran for the town before, none left to notice a reopen itself
    town.reopen();
    let mut failed = Vec::new();
    // how a resident is written, before any resident, so every one can be written with it
    if let Err(e) = crate::lua::resident(lua) {
        failed.push(("resident".to_string(), e.to_string()));
    }
    let ran = comes(lua, town, &files(paths), failed);
    let came: Vec<PathBuf> = ran.iter().map(|(f, _)| f.clone()).collect();
    holding(town, &came, |_| true);
    ran
}

/// What was held of what one opening `saved` is kept only for those of the files that `came` yet
/// to ask for it: a program, and those it speaks for, until it says what it is; what a Lua file
/// joined, until it has heard its greeting, as which it joins, once its task has run, a turn after
/// this. The rest is let go, as all else that will come again has.
fn holding(town: &Rc<Town>, came: &[PathBuf], saved: impl Fn(&str) -> bool) {
    let program = |kind: &str| came.iter().any(|p| runnable(p) && under(kind, &fact(p)));
    let lua: Vec<String> = came.iter().filter(|p| !runnable(p)).map(|p| fact(p)).collect();
    let joined = move |kind: &str| lua.iter().any(|at| kind != at && under(kind, at));
    town.hold_only(|kind| !saved(kind) || program(kind) || joined(kind));
    let town = town.clone();
    tokio::task::spawn_local(async move { town.hold_only(|kind| !joined(kind)) });
}

/// Each of `files` comes, in order: a program runs, a Lua file is read. What `failed` is said
/// last. Each file, and the number of who it is.
fn comes(
    lua: &Lua,
    town: &Rc<Town>,
    files: &[PathBuf],
    mut failed: Vec<(String, String)>,
) -> Vec<(PathBuf, Option<u64>)> {
    let mut ran = Vec::new();
    for path in files {
        if runnable(path) {
            let it = who(town, path);
            ran.push((path.clone(), Some(it.id)));
            let id = it.id;
            town.runs(id, tokio::task::spawn_local(residing(town.clone(), it, path.clone())));
        } else {
            match wire(lua, town, path) {
                Ok(id) => ran.push((path.clone(), Some(id))),
                Err(e) => {
                    ran.push((path.clone(), None));
                    failed.push((named(path), e.to_string()));
                }
            }
        }
    }
    for (from, message) in failed {
        town.error(&town.who(&from, false), None, message);
    }
    ran
}

/// Where the fact of the resident a file is would be.
fn fact(path: &Path) -> String {
    format!("{RESIDENTS}/{}", whole(path).to_string_lossy().trim_start_matches('/'))
}

/// Whether `kind` is `at`, or under it.
fn under(kind: &str, at: &str) -> bool {
    kind.strip_prefix(at).is_some_and(|r| r.is_empty() || r.starts_with('/'))
}

/// Whether a file is a resident: a Lua file, or a program, wherever it is in the town's folders, so
/// a build is kept outside them and its program linked in. Never a hidden file (`.git`, an editor's
/// `.#lock`), nor an editor's backup (`name~`).
fn resident(path: &Path) -> bool {
    let name = path.file_name().map(|n| n.to_string_lossy()).unwrap_or_default();
    let named = !name.starts_with('.') && !name.ends_with('~');
    named && (path.extension().is_some_and(|x| x == "lua") || runnable(path))
}

/// A program: a file town may run that is not Lua.
fn runnable(path: &Path) -> bool {
    path.extension().is_none_or(|x| x != "lua") && crate::program::runs(path)
}

/// The residents of a town, in the order they open: each path as given; a folder's as a `Walk`
/// finds them, so a library beside them comes first; each folder read once, though two paths hold
/// it. A file named twice runs once, where it first comes.
fn files(paths: &[PathBuf]) -> Vec<PathBuf> {
    let (mut walk, mut files) = (Walk::default(), Vec::new());
    for p in paths {
        if p.is_dir() {
            walk.read(p, p, Path::new(""));
            files.extend(walk.files.drain(..).map(|(f, ..)| f).filter(|f| resident(f)));
        } else {
            files.push(p.clone());
        }
    }
    let mut ran = HashSet::new();
    files.retain(|f| ran.insert(f.canonicalize().unwrap_or(f.clone())));
    files
}

/// Open a town, and keep it to open again: say its paths, whole, at `town/residents`, and open. Its
/// `Opening` answers a future `town/open` (`work::begin`), asked by a resident that watches, as
/// `reload` does. Each open is in a new Lua, `made`.
pub fn keep(made: fn() -> Lua, town: &Rc<Town>, paths: Vec<PathBuf>) {
    let paths: Vec<PathBuf> = paths.iter().map(|p| whole(p)).collect();
    let said: Vec<Parts> = paths.iter().map(|p| p.to_string_lossy().as_ref().into()).collect();
    town.talk(Word::new(RESIDENTS, Tense::Present, Body::of(Parts::List(said))));
    let opening = Rc::new(Opening {
        made,
        lua: Default::default(),
        paths,
        ran: Default::default(),
        asked: Default::default(),
        settling: Default::default(),
    });
    *town.opening.borrow_mut() = Some(opening.clone());
    opening.open(town);
}

/// What a town keeps to open again: how its Lua is made, the Lua of this opening, its paths, each
/// file it ran and the number of who it is, the files one save asks to open, and, while its opens
/// come, when the last came and the most it waits. Town's own, so it outlives every opening.
pub struct Opening {
    made: fn() -> Lua,
    lua: RefCell<Option<Lua>>,
    paths: Vec<PathBuf>,
    ran: RefCell<Vec<(PathBuf, Option<u64>)>>,
    asked: RefCell<Vec<PathBuf>>,
    settling: Cell<Option<(Instant, Instant)>>,
}

impl Opening {
    /// A future `town/open`: one that `matters` opens what one save asks, once its opens stop.
    /// They come a few ms apart, so it opens once none has come for `SETTLE`, or a second after
    /// the first, if they never stop; every open meanwhile is of that save.
    pub(crate) fn ask(self: &Rc<Self>, town: &Rc<Town>, body: &Body) {
        let Some(path) = matters(body, &self.ran.borrow()) else { return };
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
        let (me, town) = (self.clone(), town.clone());
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
            me.again(&town, &asked);
        });
    }

    /// What one save asked opens again: each resident saved goes and comes again, alone; one
    /// deleted goes. A library, which every resident after it is written with, opens the whole
    /// town again; so does a Lua file it has not opened, one new or one that failed, which may be
    /// one; and any save before the town has opened.
    fn again(&self, town: &Rc<Town>, asked: &[PathBuf]) {
        let Some(lua) = self.lua.borrow().clone() else { return self.open(town) };
        let ran = self.ran.borrow().clone();
        let who = |path: &PathBuf| ran.iter().find(|(f, _)| f == path).and_then(|(_, id)| *id);
        // a library, as far as the town knows: a Lua file that left no fact, or that it never
        // opened, as one new, or one that failed; and one that is one now opens it all, below
        let of_lua = |path: &PathBuf| path.extension().is_some_and(|x| x == "lua");
        let library = |path: &PathBuf| who(path).is_none_or(|id| town.place_of(id).is_none());
        if asked.iter().any(|p| of_lua(p) && library(p)) {
            return self.open(town);
        }
        for path in asked {
            if let Some(id) = who(path) {
                town.leaves(&fact(path), id);
            }
        }
        // what is there: one deleted only goes
        let coming: Vec<PathBuf> =
            asked.iter().filter(|p| resident(p) && p.exists()).cloned().collect();
        let came = comes(&lua, town, &coming, Vec::new());
        if came.iter().any(|(p, id)| of_lua(p) && id.is_some_and(|id| town.place_of(id).is_none()))
        {
            return self.open(town);
        }
        holding(town, &coming, |kind| asked.iter().any(|p| under(kind, &fact(p))));
        let mut ran = self.ran.borrow_mut();
        ran.retain(|(f, _)| !asked.contains(f));
        ran.extend(came);
    }

    /// The town opened in a new Lua, so a resident keeps nothing but what the present holds; the
    /// old Lua let go with the residents it held. What it ran, kept.
    fn open(&self, town: &Rc<Town>) {
        let lua = (self.made)();
        if let Err(e) = crate::lua::install(&lua, town) {
            town.error(&Resident::town(), None, format!("cannot make Lua: {e}"));
        }
        let ran = open(&lua, town, &self.paths);
        *self.lua.borrow_mut() = Some(lua);
        *self.ran.borrow_mut() = ran;
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

/// The file a `town/open` asks to open, whole, if it matters: it is a resident, or was one among
/// those the town `ran`. One gone that it never ran, as one deleted while it was down, does not.
fn matters(body: &Body, ran: &[(PathBuf, Option<u64>)]) -> Option<PathBuf> {
    let path = whole(Path::new(body.part("path").and_then(Parts::text)?));
    let is = resident(&path) && path.exists();
    (is || ran.iter().any(|(f, _)| *f == path)).then_some(path)
}

/// Who a file is, as it comes: by its name, at its whole path.
pub(crate) fn who(town: &Town, path: &Path) -> Resident {
    Resident { file: Some(whole(path)), ..town.who(&named(path), false) }
}

/// What a resident is called: its file's name, but for `.lua`.
pub(crate) fn named(path: &Path) -> String {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or("?".into());
    name.strip_suffix(".lua").map(String::from).unwrap_or(name)
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
    use crate::testing::{an_error, quiet, run, scratch, until, upto, Scratch};
    use crate::town::Listening;

    /// a word of `kind`, asked for
    pub(crate) fn word(kind: &str) -> Word {
        Word::new(kind, Tense::Future, Body::none())
    }

    /// End what runs for a town by its tasks alone, not as a reopen does, so what lets go as a
    /// task ends is seen to.
    pub(crate) fn end(town: &Town) {
        town.tasks.borrow_mut().drain(..).for_each(|(_, h)| h.abort());
    }

    /// a Lua resident, wired as a town opens it, and here until the town opens again
    pub(crate) fn wired(town: &Rc<Town>, path: &Path) {
        let lua = Lua::new();
        crate::lua::install(&lua, town).unwrap();
        crate::lua::resident(&lua).unwrap();
        wire(&lua, town, path).unwrap();
    }

    /// a resident that is the language `name`, its grammar in Lua: a file in `dir`, `wired`
    pub(crate) fn speaking(town: &Rc<Town>, dir: &Path, name: &str, grammar: &str) {
        let path = dir.join(format!("{name}.lua"));
        std::fs::write(&path, format!("return {{ languages = {{ {name} = {grammar} }} }}"))
            .unwrap();
        wired(town, &path);
    }

    /// a shell script in `dir`, a program town may run
    pub(crate) fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    // a folder's own files come before its folders, so a library beside them is there first; a
    // resident, however deep; a linked folder read like any other, once, so a link back up ends; no
    // hidden file or folder, no backup, nothing that is no resident
    #[test]
    fn a_folder_reads_its_files_then_its_folders() {
        let dir = scratch();
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
            .iter()
            .map(|p| p.strip_prefix(dir.join("town")).unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["lib.lua", "run", "apps/deep", "apps/echo.lua", "b-linked/linked.lua"]);
        // a file named by itself and in its folder runs once, where it first comes
        let twice = files(&[dir.join("town/apps/echo.lua"), dir.join("town")]);
        assert_eq!(twice.len(), 5);
        assert!(twice[0].ends_with("apps/echo.lua"));
    }

    // an open matters to a resident, however deep, and only to one: not to an editor's backup or
    // lock, or what is no resident; a resident gone, or no longer runnable, matters, as it was one;
    // and an open names a path
    #[test]
    fn only_an_open_of_a_resident_matters() {
        use std::os::unix::fs::PermissionsExt;
        let dir = scratch();
        let top = dir.join("town");
        std::fs::create_dir_all(top.join("target")).unwrap();
        std::fs::create_dir_all(top.join("apps")).unwrap();
        std::fs::write(top.join("apps/a.lua"), "").unwrap();
        let shout = script(&top, "shout.py", "");
        let ask = script(&top, "ask.sh", "");
        let read: Vec<_> =
            files(std::slice::from_ref(&top)).into_iter().map(|f| (f, None)).collect();
        let changed = |path: &Path| matters(&asked(path), &read).is_some();
        std::fs::write(top.join("Cargo.toml"), "").unwrap();
        script(&top, "run~", "");
        assert!(!changed(&top.join("Cargo.toml")) && !changed(&top.join("run~")), "no resident");
        assert!(!changed(&top.join(".#lock")), "an editor's lock");
        assert!(changed(&script(&top.join("target"), "built", "")), "a resident, however deep");
        assert!(changed(&top.join("apps/a.lua")), "a resident saved");
        std::fs::set_permissions(&ask, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(changed(&ask), "a program no longer runnable");
        std::fs::remove_file(&shout).unwrap();
        assert!(changed(&shout), "a resident taken away, whatever it is named");
        assert!(matters(&Body::none(), &read).is_none(), "an open names a path");
    }

    // a path is one place, read as a path is, so each part of it names something
    #[test]
    fn a_path_is_made_whole() {
        assert_eq!(whole(Path::new("/a/./b/")), Path::new("/a/b"));
        assert_eq!(whole(Path::new("/a/c/../b")), Path::new("/a/b"));
        assert_eq!(whole(Path::new("b")), std::env::current_dir().unwrap().join("b"));
    }

    /// an open of `path`, as a resident asks it
    fn asked(path: &Path) -> Body {
        Body::of(Parts::map([("path", path.to_string_lossy().as_ref())]))
    }

    /// a Lua file that says it opened, as it is read, so each opening is heard, however soon the
    /// next; and a town kept from `top`, heard from before it opens
    async fn kept(dir: &Scratch, top: &Path) -> (Rc<Town>, Listening) {
        std::fs::create_dir_all(top).unwrap();
        std::fs::write(top.join("opens.lua"), r#"talk(event("opened"))"#).unwrap();
        let town = Town::with_log(dir.log());
        let mut words = town.words.listen();
        keep(Lua::new, &town, vec![top.join(".")]);
        until(&mut words, |w| w.kind == "opened").await;
        (town, words)
    }

    /// whether the town opens again within a while
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
            assert_eq!(paths.value(), serde_json::json!([top]), "the paths, whole");
            town.reopen();
        });
    }

    // a Lua resident keeps nothing across an open but what the present holds: its globals are
    // new each time
    #[test]
    fn a_lua_resident_keeps_no_globals_across_an_open() {
        run(async {
            let dir = scratch();
            let top = dir.join("town");
            std::fs::create_dir_all(&top).unwrap();
            // a resident, which comes again alone, and a library, which opens the whole town
            let count = top.join("count.lua");
            let counts = r#"n = (n or 0) + 1; talk(event("counted", n))"#;
            std::fs::write(
                &count,
                format!("{counts}; return {{ kinds = {{ \"x\" }}, listen = function() end }}"),
            )
            .unwrap();
            let lib = top.join("a-lib.lua");
            std::fs::write(&lib, r#"m = (m or 0) + 1; talk(event("libbed", m))"#).unwrap();
            let town = Town::with_log(dir.log());
            let mut words = town.words.listen();
            keep(Lua::new, &town, vec![top]);
            let counted = async |words: &mut Listening, kind: &'static str| {
                until(words, move |w| w.kind == kind).await.body.value()
            };
            assert_eq!(counted(&mut words, "counted").await, 1);
            town.talk(Word::new("town/open", Tense::Future, asked(&count)));
            assert_eq!(counted(&mut words, "counted").await, 1, "a resident begins again");
            town.talk(Word::new("town/open", Tense::Future, asked(&lib)));
            assert_eq!(counted(&mut words, "libbed").await, 1, "and a library");
            town.reopen();
        });
    }

    // a file saved alone is given the libraries it came after the first time, and no later one
    #[test]
    fn a_resident_saved_alone_has_only_the_libraries_before_it() {
        run(async {
            let dir = scratch();
            let top = dir.join("town");
            std::fs::create_dir_all(&top).unwrap();
            std::fs::write(top.join("a-early.lua"), "early = true").unwrap();
            let middle = top.join("b-middle.lua");
            let says = r#"talk(event("knows", tostring(early) .. " " .. tostring(late)))"#;
            std::fs::write(
                &middle,
                format!("{says}; return {{ kinds = {{ \"x\" }}, listen = function() end }}"),
            )
            .unwrap();
            std::fs::write(top.join("c-late.lua"), "late = true").unwrap();
            let town = Town::with_log(dir.log());
            let mut words = town.words.listen();
            keep(Lua::new, &town, vec![top]);
            let knows = async |words: &mut Listening| {
                until(words, |w| w.kind == "knows").await.body.value()
            };
            assert_eq!(knows(&mut words).await, "true nil", "as it first came");
            town.talk(Word::new("town/open", Tense::Future, asked(&middle)));
            assert_eq!(knows(&mut words).await, "true nil", "and as it comes again alone");
            town.reopen();
        });
    }

    // a town done is freed: its Lua and its joins do not hold it, so once what runs for it ends
    // and it is dropped, it is gone
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
            keep(Lua::new, &town, vec![top]);
            town.talk(Word::new("go", Tense::Past, Body::none()));
            until(&mut words, |w| w.kind == "mine").await;
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
    // it is only once it runs; what a Lua file that is gone owned is let go as the town has opened
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
            let lua = Lua::new();
            open(&lua, &town, &[themer.clone(), gone.clone()]);
            let came = |w: &Word| w.kind.ends_with("/themer") && !w.body.is_none();
            until(&mut words, came).await;
            std::fs::remove_file(&gone).unwrap();
            open(&lua, &town, &[themer]);
            let owns = |own: &str| Resident { own: vec![own.into()], ..town.who("square", true) };
            assert!(town.come(&owns("theme")).is_err(), "held, before it says what it is");
            assert!(town.come(&owns("gone")).is_ok(), "a Lua file gone: let go");
            until(&mut words, came).await;
            assert!(town.come(&owns("theme")).is_err(), "its own again");
            town.reopen();
        });
    }

    // a future `town/open` of a resident opens it again, alone; of a library, which the residents
    // after it are written with, the whole town; of what is no resident, nothing; of a resident
    // deleted, it goes; and many opens of one save open once
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
            until(&mut words, |w| w.kind == "new").await;
            assert!(!opens(&mut words).await, "a new file comes, alone");
            (0..5).for_each(|_| open(&top.join("opens.lua")));
            assert!(opens(&mut words).await && !opens(&mut words).await, "once, for one save");
            assert!(town.facts(&fact(&new)).contains_key(&fact(&new)), "come again with the rest");
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
            let cat = format!("{}/cat", fact(&a));
            let joined = |w: &Word| w.kind == cat && !w.body.is_none();
            if id(&cat).is_none() {
                until(&mut words, joined).await;
            }
            let (was, its, others) = (id(&fact(&a)), id(&cat), id(&fact(&b)));
            assert!(town.joins.borrow().contains_key("cat"), "joined, under the one that asked");

            open(&a);
            until(&mut words, |w| w.kind == cat && w.body.is_none()).await;
            until(&mut words, joined).await;
            assert!(id(&fact(&a)) != was && id(&cat) != its, "it, and its join, come again");
            assert_eq!(id(&fact(&b)), others, "the others stay");
            assert!(!opens(&mut words).await, "no more opens");

            std::fs::remove_file(&a).unwrap();
            open(&a);
            until(&mut words, |w| w.kind == fact(&a) && w.body.is_none()).await;
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
            open(&Lua::new(), &town, std::slice::from_ref(&late));
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
            let ran = open(&Lua::new(), &town, &[joins.clone(), mute.clone()]);
            let cat = format!("{}/cat", fact(&joins));
            until(&mut words, |w| w.kind == cat && !w.body.is_none()).await;
            for (path, id) in &ran {
                town.leaves(&fact(path), id.unwrap());
            }
            let said = quiet(&mut words).await;
            let nothing =
                |at: &str| said.iter().filter(|w| w.kind == at && w.body.is_none()).count();
            assert_eq!((nothing(&fact(&joins)), nothing(&cat)), (1, 1), "once each");
            assert_eq!(nothing(&fact(&mute)), 0, "none where none was");
            town.reopen();
        });
    }

    // what a save is, a library or a resident, is read from the file saved: a library that failed
    // to open, and a resident saved as a library, open the whole town, so the files after them are
    // written with them
    #[test]
    fn a_save_that_is_a_library_opens_the_whole_town() {
        run(async {
            let dir = scratch();
            let top = dir.join("town");
            std::fs::create_dir_all(&top).unwrap();
            let (lib, now) = (top.join("a-lib.lua"), top.join("c-now.lua"));
            std::fs::write(&lib, "function greet(").unwrap();
            std::fs::write(top.join("b.lua"), r#"talk(event("greeted", greet()))"#).unwrap();
            std::fs::write(&now, r#"return { own = { "c" } }"#).unwrap();
            std::fs::write(top.join("d.lua"), r#"talk(event("shouted", shout()))"#).unwrap();
            let town = Town::with_log(dir.log());
            let mut words = town.words.listen();
            keep(Lua::new, &town, vec![top]);
            let open = |path: &Path| town.talk(Word::new("town/open", Tense::Future, asked(path)));
            let heard = async |words: &mut Listening, kind: &'static str| {
                let said = until(words, move |w| w.kind == kind);
                tokio::time::timeout(Duration::from_secs(2), said).await.is_ok()
            };
            std::fs::write(&lib, r#"function greet() return "hi" end"#).unwrap();
            open(&lib);
            assert!(heard(&mut words, "greeted").await, "a library that failed, mended");
            std::fs::write(&now, r#"function shout() return "!" end"#).unwrap();
            open(&now);
            assert!(heard(&mut words, "shouted").await, "a resident, now a library");
            town.reopen();
        });
    }

    // a Lua resident deleted goes, and nothing fails: it is not opened, as it is not there; and
    // one deleted that the town never ran is nothing to it
    #[test]
    fn a_lua_resident_deleted_goes_quietly() {
        run(async {
            let dir = scratch();
            let top = dir.join("town");
            std::fs::create_dir_all(&top).unwrap();
            let gone = top.join("gone.lua");
            std::fs::write(&gone, r#"return { own = { "gone" } }"#).unwrap();
            let (town, mut words) = kept(&dir, &top).await;
            let open = |path: &Path| town.talk(Word::new("town/open", Tense::Future, asked(path)));
            std::fs::remove_file(&gone).unwrap();
            open(&gone);
            let said = upto(&mut words, |w| w.kind == fact(&gone) && w.body.is_none()).await;
            let said = [said, quiet(&mut words).await].concat();
            assert!(!said.iter().any(|w| w.kind == "town/error"), "nothing fails: {:?}", said);
            open(&top.join("never.lua"));
            assert!(!opens(&mut words).await, "one never ran: nothing");
            town.reopen();
        });
    }

    // what a program held, and what was under it, is held until it says what it is again, and
    // let go then, so what it no longer asks for is not held from the square for good: saved, or
    // as the whole town opens again
    #[test]
    fn what_a_program_no_longer_asks_for_is_let_go() {
        run(async {
            let dir = scratch();
            let top = dir.join("town");
            std::fs::create_dir_all(&top).unwrap();
            let lib = top.join("a-lib.lua");
            std::fs::write(&lib, "").unwrap();
            let was = r#"echo '{"own":["p"]}'
                echo '{"kind":"town/join","tense":"future","body":{"as":"cat","argv":["cat"]}}'
                echo '{"kind":"town/residents/sub","body":{"own":["subs"]}}'
                exec sleep 60"#;
            let is = r#"echo '{"own":["p"]}'; echo '{"kind":"up","tense":"past"}'; exec sleep 60"#;
            let p = script(&top, "p", was);
            let town = Town::with_log(dir.log());
            let mut words = town.words.listen();
            keep(Lua::new, &town, vec![top.clone()]);
            let open = |path: &Path| town.talk(Word::new("town/open", Tense::Future, asked(path)));
            let owns = |own: &str| Resident { own: vec![own.into()], ..town.who("square", true) };
            let (cat, sub) = (format!("{}/cat", fact(&p)), format!("{}/sub", fact(&p)));
            for again in [&p, &lib] {
                script(&top, "p", was);
                if again == &lib {
                    open(&p);
                }
                let mut asked = [false, false];
                while asked != [true, true] {
                    let w = until(&mut words, |w| !w.body.is_none()).await;
                    asked = [asked[0] || w.kind == cat, asked[1] || w.kind == sub];
                }
                script(&top, "p", is);
                open(again);
                until(&mut words, |w| w.kind == "up").await;
                assert!(town.come(&owns("p")).is_err(), "its own, again");
                assert!(town.come(&owns("cat")).is_ok(), "its join, let go");
                assert!(town.come(&owns("subs")).is_ok(), "one it spoke for, let go");
            }
            town.reopen();
        });
    }

    // what a Lua file joined is held from the square until it has heard its greeting, as which it
    // joins again; and let go then, if it no longer does
    #[test]
    fn what_a_lua_file_joins_as_it_hears_is_held_until_it_hears() {
        run(async {
            let dir = scratch();
            let top = dir.join("town");
            std::fs::create_dir_all(&top).unwrap();
            let joins = top.join("joins.lua");
            let was = r#"return { kinds = { "town/residents" }, listen = function(w)
                if w.kind == "town/residents" then return join("cat", { "cat" }) end
              end }"#;
            std::fs::write(&joins, was).unwrap();
            let town = Town::with_log(dir.log());
            let mut words = town.words.listen();
            keep(Lua::new, &town, vec![top]);
            let cat = format!("{}/cat", fact(&joins));
            until(&mut words, |w| w.kind == cat && !w.body.is_none()).await;
            let owns = |own: &str| Resident { own: vec![own.into()], ..town.who("square", true) };
            let opening = town.opening.borrow().clone().unwrap();
            // in the turn it comes again, before it hears
            opening.again(&town, std::slice::from_ref(&joins));
            assert!(town.come(&owns("cat")).is_err(), "held, before it hears");
            until(&mut words, |w| w.kind == cat && !w.body.is_none()).await;
            std::fs::write(&joins, r#"return { own = { "j" } }"#).unwrap();
            opening.again(&town, std::slice::from_ref(&joins));
            quiet(&mut words).await;
            assert!(town.come(&owns("cat")).is_ok(), "let go, once it has heard");
            town.reopen();
        });
    }
}
