//! `mirror <path>`: says what a path contains, and again as it changes. A program like any other.
//!
//! - **what it says**: a file as a fact at `""`; a folder as a fact of each file in it, by its
//!   path from the folder, `a/b`. A fact is `{ path, changed, size }`, `changed` the system's ns.
//! - **links**: followed, but not back into the mirror, so each file is said once, where it is.
//!   Where one leads is watched, or the nearest folder there, so a file linked in is heard as it
//!   is built.
//! - **as it comes**: each file, then `now`; after, what each event names, and what went, as
//!   nothing.
//! - **events**: the filesystem's, never a tick.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc;

use notify::{RecursiveMode, Watcher};

use town::body::Body;
use town::parts::Parts;
use town::residents::Walk;
use town::voice::line;
use town::word::{hears, Tense, Word};

/// What comes to a mirror: paths changed, or all of it, when events are lost; or its town gone.
enum Comes {
    Changed(Vec<PathBuf>),
    Gone,
}

/// Mirror `path` for a town on stdin and stdout, until the town hangs up.
fn main() {
    let args: Vec<String> = std::env::args().collect();
    let [_, path] = &args[..] else {
        eprintln!("usage: mirror <path>");
        std::process::exit(2);
    };
    if let Err(e) = run(Path::new(path)) {
        eprintln!("mirror: {e}");
        std::process::exit(1);
    }
}

fn run(path: &Path) -> Result<(), String> {
    if !path.is_absolute() {
        return Err(format!("{} is no whole path: one is, from /", path.display()));
    }
    let (tx, rx) = mpsc::channel();
    // watched first, so nothing that changes while it reads is missed
    let mut watcher = watcher(path, tx.clone())?;
    // what it is told: nothing it needs, but its end is the town's
    std::thread::spawn(move || {
        for _ in std::io::stdin().lock().lines() {}
        tx.send(Comes::Gone).ok();
    });
    mirror(path, rx, &mut std::io::stdout(), &mut |lead, on| follow(&mut watcher, lead, on));
    Ok(())
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

/// Say what `path` contains, all of it first, then what each change names, until the town is gone;
/// and watch where each link in it leads, as it is found.
fn mirror(
    path: &Path,
    comes: mpsc::Receiver<Comes>,
    out: &mut impl Write,
    follow: &mut dyn FnMut(&Lead, bool),
) {
    let (mut leads, mut followed, mut told_now) = (BTreeMap::new(), HashSet::new(), false);
    // the kinds it has said, so one gone is said as nothing
    let mut said = BTreeSet::new();
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
        // the path itself, as a link is: made again in its place, it is watched anew, and read;
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
        // where no link leads now, nor the path itself, is watched no longer: a link rebuilt, to
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
        // each file said once, then `now`, as a resident is told once its present is
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

/// Where in the path what an event names is: in the path itself, there alone, as a walk finds
/// it where it truly is, though a link leads back up to it; else in each link in it that leads
/// there, by the link's path; nowhere, if it is of neither.
fn placed(p: &Path, path: &Path, real: &Path, leads: &BTreeMap<String, Lead>) -> Vec<PathBuf> {
    if let Ok(rest) = p.strip_prefix(path).or_else(|_| p.strip_prefix(real)) {
        return vec![rest.to_path_buf()];
    }
    let mut at = Vec::new();
    for (kind, lead) in leads {
        if let Ok(rest) = p.strip_prefix(&lead.at) {
            at.push(if lead.whole { Path::new(kind).join(rest) } else { PathBuf::from(kind) });
        }
    }
    at
}

/// What is now at `kind` in the path, a file, a folder of them, or nothing: each file said, and
/// each it said there before and is not there now, as nothing; and where the links there lead.
fn settle(
    watched: &Path,
    kind: &Path,
    said: &mut BTreeSet<String>,
    leads: &mut BTreeMap<String, Lead>,
    out: &mut impl Write,
) -> std::io::Result<()> {
    let at = kind.to_string_lossy().into_owned();
    let of = |k: &String| at.is_empty() || hears(&format!("/{at}"), k);
    let (now, found) = files(watched, kind, leads);
    leads.retain(|k, _| !of(k));
    leads.extend(found);
    for (k, body) in &now {
        said.insert(k.clone());
        writeln!(out, "{}", line(&Word::new(k, Tense::Present, body.clone())))?;
    }
    // what was said of `kind`: all of it, for the whole path; else it, and what is under it
    let was = said.range(at.clone()..).take_while(|k| k.starts_with(&at));
    let gone: Vec<String> = was.filter(|k| of(k) && !now.contains_key(*k)).cloned().collect();
    for k in gone {
        said.remove(&k);
        writeln!(out, "{}", line(&Word::new(k, Tense::Present, Body::none())))?;
    }
    out.flush()
}

/// Where a link in the path leads, watched so a change there is heard: a folder, whole; a file,
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

/// Every file at `kind` in the path, however deep, by its kind, as `{ path, changed, size
/// }`, and where each link there leads. A part is read as the whole is: a folder that a link
/// outside `kind` leads to is read there, not here.
fn files(
    watched: &Path,
    kind: &Path,
    leads: &BTreeMap<String, Lead>,
) -> (BTreeMap<String, Body>, BTreeMap<String, Lead>) {
    use std::os::unix::fs::MetadataExt;
    let mut walk = Walk::default();
    // what is hidden is not of the path, however it is named: read in a folder, or by an event
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

/// A path as the system keeps it, as events name it: /private/var/x for /var/x; where it is not
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

    // a file changed is said changed, its size the same and its time set back: the system's own
    // clock for it moves however it is changed
    #[test]
    fn a_change_of_the_same_size_is_heard() {
        let dir = town::testing::scratch();
        std::fs::write(dir.join("a"), "dark").unwrap();
        let before = files(&dir, Path::new("a"), &BTreeMap::new()).0;
        let then = std::fs::metadata(dir.join("a")).unwrap().modified().unwrap();
        std::fs::write(dir.join("a"), "lite").unwrap();
        let file = std::fs::File::options().write(true).open(dir.join("a")).unwrap();
        file.set_modified(then).unwrap(); // as `cp -p` would
        assert_ne!(files(&dir, Path::new("a"), &BTreeMap::new()).0, before);
    }

    // what is mirrored is a whole path, one place wherever town runs; and one that is a link is
    // followed
    #[test]
    fn a_mirrored_path_is_whole_and_followed() {
        assert!(run(Path::new("colors")).unwrap_err().contains("no whole path"));
        let dir = town::testing::scratch();
        std::fs::create_dir_all(dir.join("theme")).unwrap();
        std::fs::write(dir.join("theme/colors"), "bg=#000").unwrap();
        std::os::unix::fs::symlink(dir.join("theme"), dir.join("link")).unwrap();
        let kinds: Vec<String> =
            files(&dir.join("link"), Path::new(""), &BTreeMap::new()).0.into_keys().collect();
        assert_eq!(kinds, ["colors"]);
    }
}
