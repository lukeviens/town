//! `town-watch <path>`: a program that says what a path holds, and again as it changes. Town's own,
//! a program like any other: town itself watches nothing.
//!
//! - **joined**: `join(kind, { "town-watch", path })`. It is told the facts town has there, then
//!   `now`.
//! - **what it says**: a file as a fact at `""`, the kind itself; a folder as a fact of each file
//!   in it, however deep, by its path from the folder, `a/b`.
//! - **a fact**: `{ path, changed, size }`: its whole path; when it last changed, by the system's
//!   own clock for it, in ns since 1970, which none can set back; and its size. What it holds stays
//!   on the disk.
//! - **links**: followed, but not into the watch itself, so each file is said once, where it truly
//!   is, and a link back up is not read again. Where one leads is watched, or, not there yet, the
//!   nearest folder there is, so a file built elsewhere and linked in is heard as it is built.
//! - **changes**: what differs from what town has is said, and what is gone as nothing, so a town
//!   opened again hears only what changed.
//! - **events**: the filesystem's, never a tick. A save that takes a file away a moment is said as
//!   it happened, gone and then back.

use std::collections::{BTreeMap, HashSet};
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc;

use notify::{RecursiveMode, Watcher};

use town::body::Body;
use town::parts::Parts;
use town::residents::Walk;
use town::voice::{line, read};
use town::word::{hears, Tense, Word};

/// What comes to a watch: paths changed, or all of it, when events are lost; or its town gone.
enum Comes {
    Changed(Vec<PathBuf>),
    Gone,
}

/// Watch `path` for a town on stdin and stdout, until the town hangs up.
fn main() {
    let args: Vec<String> = std::env::args().collect();
    let [_, path] = &args[..] else {
        eprintln!("usage: town-watch <path>");
        std::process::exit(2);
    };
    if let Err(e) = run(Path::new(path)) {
        eprintln!("town-watch: {e}");
        std::process::exit(1);
    }
}

fn run(path: &Path) -> Result<(), String> {
    if !path.is_absolute() {
        return Err(format!("{} is no whole path: one is, from /", path.display()));
    }
    let (tx, rx) = mpsc::channel();
    // watched first, so nothing that changes while it is told what town has is missed
    let mut watcher = watcher(path, tx.clone())?;
    let said = greeting(&mut std::io::stdin().lock());
    // what it is told after: nothing it needs, but its end is the town's
    std::thread::spawn(move || {
        for _ in std::io::stdin().lock().lines() {}
        tx.send(Comes::Gone).ok();
    });
    watch(path, said, rx, &mut std::io::stdout(), &mut |lead, on| follow(&mut watcher, lead, on));
    Ok(())
}

/// What town has of the watch: each fact it tells, by its path, until `now`.
fn greeting(told: &mut impl BufRead) -> BTreeMap<String, Body> {
    let mut said = BTreeMap::new();
    for l in told.lines().map_while(Result::ok) {
        match read(&l) {
            Ok(w) if w.is_now() => break,
            Ok(w) if w.tense == Tense::Present => {
                said.insert(w.kind, w.body);
            }
            _ => {}
        }
    }
    said
}

/// The filesystem's events for `path`: a file through its folder, since a save that replaces it
/// would end a watch on the file; a folder, whole; where neither is yet, the nearest folder there
/// is, so its coming is heard. An event that names no path, or says events were lost, is all of it.
fn watcher(path: &Path, tx: mpsc::Sender<Comes>) -> Result<impl Watcher, String> {
    let dir = path.is_dir();
    let at = if dir { path } else { path.parent().unwrap_or(path) };
    let at = at.ancestors().find(|a| a.exists()).unwrap_or(at);
    let whole = path.to_path_buf();
    let mut watcher = notify::recommended_watcher(move |e: notify::Result<notify::Event>| {
        let comes = match e {
            Ok(e) if !e.need_rescan() && !e.paths.is_empty() => e.paths,
            _ => vec![whole.clone()],
        };
        tx.send(Comes::Changed(comes)).ok();
    })
    .map_err(|e| format!("cannot watch {}: {e}", path.display()))?;
    let deep = if dir { RecursiveMode::Recursive } else { RecursiveMode::NonRecursive };
    watcher.watch(at, deep).map_err(|e| format!("cannot watch {}: {e}", path.display()))?;
    Ok(watcher)
}

/// Watch where a link leads, or, `on` false, no longer; one that cannot be watched is let be.
fn follow(watcher: &mut impl Watcher, lead: &Lead, on: bool) {
    let deep = if lead.whole { RecursiveMode::Recursive } else { RecursiveMode::NonRecursive };
    match on {
        true => watcher.watch(&lead.at, deep).ok(),
        false => watcher.unwatch(&lead.at).ok(),
    };
}

/// Say what `path` holds where it differs from what was said, all of it first, then what each
/// change names, until the town is gone; and watch where each link in it leads, as it is found.
fn watch(
    path: &Path,
    mut said: BTreeMap<String, Body>,
    comes: mpsc::Receiver<Comes>,
    out: &mut impl Write,
    follow: &mut dyn FnMut(&Lead, bool),
) {
    let (mut leads, mut followed, mut told_now) = (BTreeMap::new(), HashSet::new(), false);
    let mut changed = vec![PathBuf::new()];
    loop {
        for kind in &changed {
            if settle(path, kind, &mut said, &mut leads, out).is_err() {
                return; // town hung up
            }
        }
        // what a link leads to, watched as it is found; then read again, now it is watched, since
        // what came there before it was is not heard
        let mut new = Vec::new();
        // the watch itself, as a link is: made again in its place, it is watched anew, and read;
        // while it is gone, the nearest folder there is, so its coming back is heard
        let (dir, real) = (path.is_dir(), held(path));
        let at: PathBuf = if dir { real.clone() } else { real.parent().unwrap_or(&real).into() };
        let gone = !at.exists();
        let root = match gone {
            false => Lead::to(at, dir),
            true => Lead::to(at.ancestors().find(|a| a.exists()).unwrap_or(&at).into(), false),
        };
        if followed.insert(root.clone()) && told_now {
            follow(&root, true);
            new.push(PathBuf::new());
        }
        for (kind, lead) in &leads {
            if followed.insert(lead.clone()) {
                follow(lead, true);
                new.push(PathBuf::from(kind));
            }
        }
        // where no link leads now, nor the watch itself, is watched no longer: a link rebuilt, to
        // a new place each time, would otherwise leave a watch behind each time
        let now: HashSet<&Lead> = leads.values().chain([&root]).collect();
        let left: Vec<Lead> = followed.iter().filter(|l| !now.contains(l)).cloned().collect();
        for lead in left {
            followed.remove(&lead);
            if !now.iter().any(|l| l.at == lead.at) {
                follow(&lead, false);
            }
        }
        if !new.is_empty() {
            changed = new;
            continue;
        }
        // all it holds said once, then `now`, as a resident is told once its present is
        if !told_now {
            told_now = true;
            if writeln!(out, "{}", line(&Word::now())).and_then(|_| out.flush()).is_err() {
                return;
            }
        }
        // what comes while it reads: once each, by its kind
        let mut more = match comes.recv() {
            Ok(Comes::Changed(more)) => more,
            Ok(Comes::Gone) | Err(_) => return,
        };
        while let Ok(also) = comes.try_recv() {
            match also {
                Comes::Changed(also) => more.extend(also),
                Comes::Gone => return,
            }
        }
        // while where it is is gone, whatever comes there may be it back: all of it, read again
        changed = match gone {
            true => vec![PathBuf::new()],
            false => more.iter().flat_map(|p| placed(p, path, &held(path), &leads)).collect(),
        };
        changed.sort();
        changed.dedup();
    }
}

/// Where in the watch what an event names is: in the watch itself, or in a link in it, by
/// the link's path; nowhere, if it is of neither.
fn placed(p: &Path, path: &Path, real: &Path, leads: &BTreeMap<String, Lead>) -> Vec<PathBuf> {
    let mut at: Vec<PathBuf> = p
        .strip_prefix(path)
        .or_else(|_| p.strip_prefix(real))
        .ok()
        .map(Path::to_path_buf)
        .into_iter()
        .collect();
    for (kind, lead) in leads {
        if let Ok(rest) = p.strip_prefix(&lead.at) {
            at.push(if lead.whole { Path::new(kind).join(rest) } else { PathBuf::from(kind) });
        }
    }
    at
}

/// What is now at `kind` in the watch, a file, a folder of them, or nothing, said where it differs
/// from what was said of it; and where the links there lead.
fn settle(
    watched: &Path,
    kind: &Path,
    said: &mut BTreeMap<String, Body>,
    leads: &mut BTreeMap<String, Lead>,
    out: &mut impl Write,
) -> std::io::Result<()> {
    let at = kind.to_string_lossy().into_owned();
    let of = |k: &String| at.is_empty() || hears(&format!("/{at}"), k);
    let (now, found) = files(watched, kind, leads);
    leads.retain(|k, _| !of(k));
    leads.extend(found);
    for (k, body) in &now {
        if said.get(k) != Some(body) {
            said.insert(k.clone(), body.clone());
            writeln!(out, "{}", line(&Word::new(k, Tense::Present, body.clone())))?;
        }
    }
    // what was said of `kind`: all of it, for the whole watch; else it, and what is under it
    let was = said.range(at.clone()..).take_while(|(k, _)| k.starts_with(&at));
    let gone: Vec<String> =
        was.map(|(k, _)| k).filter(|k| of(k) && !now.contains_key(*k)).cloned().collect();
    for k in gone {
        said.remove(&k);
        writeln!(out, "{}", line(&Word::new(k, Tense::Present, Body::none())))?;
    }
    out.flush()
}

/// Where a link in the watch leads, watched so a change there is heard: a folder, whole; a file,
/// through its folder; one that leads nowhere yet, at the nearest folder there is. `which` tells a
/// folder made again in its place from the one before, to watch it anew: a watch may end with its
/// folder, as inotify's does.
#[derive(Clone, PartialEq, Eq, Hash)]
struct Lead {
    at: PathBuf,
    whole: bool,
    which: Option<u64>,
}

impl Lead {
    fn to(at: PathBuf, whole: bool) -> Lead {
        use std::os::unix::fs::MetadataExt;
        let which = std::fs::metadata(&at).ok().map(|m| m.ino());
        Lead { at, whole, which }
    }
}

/// Every file at `kind` in the watch, however deep, by its kind, as `{ path, changed, size
/// }`, and where each link there leads. A part is read as the whole is: a folder that a link
/// outside `kind` leads to is read there, not here.
fn files(
    watched: &Path,
    kind: &Path,
    leads: &BTreeMap<String, Lead>,
) -> (BTreeMap<String, Body>, BTreeMap<String, Lead>) {
    use std::os::unix::fs::MetadataExt;
    let mut walk = Walk::default();
    // what is hidden is not of the watch, however it is named: read in a folder, or by an event
    if kind.components().any(|part| part.as_os_str().to_string_lossy().starts_with('.')) {
        return (BTreeMap::new(), BTreeMap::new());
    }
    let start = match kind.as_os_str().is_empty() {
        true => watched.to_path_buf(),
        false => {
            let at = kind.to_string_lossy();
            let elsewhere = leads.iter().filter(|(k, _)| !hears(&format!("/{at}"), k));
            let folders = elsewhere.filter(|(_, l)| l.whole).map(|(_, l)| l.at.clone());
            walk.folders.extend(folders.chain(watched.canonicalize().ok()));
            watched.join(kind)
        }
    };
    walk.read(watched, &start, kind);
    let mut out = BTreeMap::new();
    for (path, kind, meta) in walk.files {
        let changed = meta.ctime().saturating_mul(1_000_000_000).saturating_add(meta.ctime_nsec());
        let mut file = Parts::map([("path", path.to_string_lossy().as_ref())]);
        file.set("changed", changed);
        file.set("size", meta.len());
        out.insert(kind.to_string_lossy().into_owned(), Body::of(file));
    }
    let leads = walk.leads.into_iter().map(|(k, (at, whole))| (k, Lead::to(at, whole))).collect();
    (out, leads)
}

/// A path as the system holds it, as events name it: /private/var/x for /var/x; where it is not
/// yet, as near as there is, and the rest as it is named.
fn held(path: &Path) -> PathBuf {
    let near =
        path.ancestors().find_map(|a| Some((a.canonicalize().ok()?, path.strip_prefix(a).ok()?)));
    match near {
        Some((at, rest)) if rest.as_os_str().is_empty() => at,
        Some((at, rest)) => at.join(rest),
        None => path.to_path_buf(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // what town has is told until the watch is joined: each fact, by its path, and no other word
    #[test]
    fn a_greeting_is_what_town_has() {
        let told = [
            r#"{"kind":"","tense":"present","body":"root"}"#,
            r#"{"kind":"a/b","tense":"present","body":"x"}"#,
            r#"{"kind":"c","tense":"future","body":"no"}"#,
            r#"{"kind":"","tense":"past","body":null}"#,
            r#"{"kind":"town/after","tense":"present","body":"no"}"#,
        ]
        .join("\n");
        let said = greeting(&mut told.as_bytes());
        assert_eq!(
            said.into_iter().collect::<Vec<_>>(),
            [("".into(), Body::of("root")), ("a/b".into(), Body::of("x"))]
        );
    }

    // a file changed is said changed, its size the same and its time set back: the system's own
    // clock for it moves however it is changed
    #[test]
    fn a_change_of_the_same_size_is_heard() {
        let dir = std::env::temp_dir().join(format!("town-same-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a"), "dark").unwrap();
        let before = files(&dir, Path::new("a"), &BTreeMap::new()).0;
        let then = std::fs::metadata(dir.join("a")).unwrap().modified().unwrap();
        std::fs::write(dir.join("a"), "lite").unwrap();
        let file = std::fs::File::options().write(true).open(dir.join("a")).unwrap();
        file.set_modified(then).unwrap(); // as `cp -p` would
        assert_ne!(files(&dir, Path::new("a"), &BTreeMap::new()).0, before);
        std::fs::remove_dir_all(&dir).ok();
    }

    // a watch is a whole path, one place wherever town runs; and one that is a link is followed
    #[test]
    fn a_watch_is_a_whole_path_followed() {
        assert!(run(Path::new("colors")).unwrap_err().contains("no whole path"));
        let dir = std::env::temp_dir().join(format!("town-linked-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("theme")).unwrap();
        std::fs::write(dir.join("theme/colors"), "bg=#000").unwrap();
        std::os::unix::fs::symlink(dir.join("theme"), dir.join("link")).unwrap();
        let kinds: Vec<String> =
            files(&dir.join("link"), Path::new(""), &BTreeMap::new()).0.into_keys().collect();
        assert_eq!(kinds, ["colors"]);
        std::fs::remove_dir_all(&dir).ok();
    }

    // a watch says each file, any bytes, as where it is and when it changed: what differs from
    // what was said, and what was said that is gone as nothing; then that what follows is now
    #[test]
    fn a_watch_says_what_differs() {
        let dir = std::env::temp_dir().join(format!("town-differs-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("a"), b"1").unwrap();
        std::fs::write(dir.join("sub/b"), [0xff, 0xfe]).unwrap();
        let file =
            |name: &str| files(&dir, Path::new(name), &BTreeMap::new()).0.remove(name).unwrap();
        let said: BTreeMap<String, Body> =
            [("a", file("a")), ("old", Body::of("x"))].map(|(k, v)| (k.to_string(), v)).into();
        let (tx, rx) = mpsc::channel();
        tx.send(Comes::Gone).unwrap();
        let mut out = Vec::new();
        watch(&dir, said, rx, &mut out, &mut |_, _| {});
        let words: Vec<(String, Body)> = String::from_utf8(out)
            .unwrap()
            .lines()
            .map(|l| read(l).map(|w| (w.kind, w.body)).unwrap())
            .collect();
        let said = [("sub/b".into(), file("sub/b")), ("old".into(), Body::none())];
        assert_eq!(
            words,
            [&said[..], &[("".into(), Body::none())]].concat(),
            "then that it is now"
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}
