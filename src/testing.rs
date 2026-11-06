//! What tests share, within town and beside it.
//!
//! - **a runtime**, **a folder** and **a world** of their own, and **scripts** for a town to run.
//! - **waiting** for a word, never for a span of time, under a guard, so a test that would hang
//!   fails; and **settling**, by a probe, until all is said.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use crate::body::Body;
use crate::town::{Listening, Town, RESIDENTS};
use crate::word::{Tense, Word};

/// how long a test that went wrong waits before it fails; no test that goes right waits on it
pub const GUARD: Duration = Duration::from_secs(30);

/// what `test` comes to, run as town runs: on one thread, its tasks beside it
pub fn run<T>(test: impl Future<Output = T>) -> T {
    Runtime::default().run(test)
}

/// A runtime kept, as town runs: its tasks go on each time it runs, so a test may run a step at a
/// time.
pub struct Runtime(tokio::runtime::Runtime, tokio::task::LocalSet);

impl Default for Runtime {
    fn default() -> Runtime {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        Runtime(rt, tokio::task::LocalSet::new())
    }
}

impl Runtime {
    /// what `f` comes to, and its tasks with it
    pub fn run<T>(&self, f: impl Future<Output = T>) -> T {
        self.1.block_on(&self.0, f)
    }
}

/// A folder of one test's own, gone as it ends.
pub struct Scratch(PathBuf);

/// a folder no other test has
pub fn scratch() -> Scratch {
    static N: AtomicU64 = AtomicU64::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("town-test-{}-{n}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    Scratch(dir)
}

impl Scratch {
    /// a log within it, for `Town::with_log`
    pub fn log(&self) -> PathBuf {
        self.0.join("past.log")
    }
}

impl std::ops::Deref for Scratch {
    type Target = Path;
    fn deref(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).ok();
    }
}

/// what `f` comes to; one that never comes fails after `GUARD`
pub async fn guard<T>(f: impl Future<Output = T>) -> T {
    tokio::time::timeout(GUARD, f).await.expect("never came")
}

/// the first word that `is`, as the town says it
pub async fn until(words: &mut Listening, is: impl Fn(&Word) -> bool) -> Word {
    guard(async {
        loop {
            match words.recv().await {
                Some(w) if is(&w) => return w,
                Some(_) => {}
                None => panic!("the town closed"),
            }
        }
    })
    .await
}

/// Every word the town says until it is quiet: each task runs as far as it can, and no word waits.
/// A guess, by yields, not a word that proves it: town's own tests wait for words instead.
pub async fn quiet(words: &mut Listening) -> Vec<Word> {
    let mut said = Vec::new();
    loop {
        for _ in 0..16 {
            tokio::task::yield_now().await;
        }
        let before = said.len();
        said.extend(std::iter::from_fn(|| words.try_recv().ok()));
        if said.len() == before {
            return said;
        }
    }
}

/// every word the town says, up to and including the first that `is`
pub async fn upto(words: &mut Listening, is: impl Fn(&Word) -> bool) -> Vec<Word> {
    let mut said: Vec<Word> = Vec::new();
    while said.last().is_none_or(|w| !is(w)) {
        said.push(until(words, |_| true).await);
    }
    said
}

/// a `town/error` whose message contains `of`
pub fn an_error(of: &'static str) -> impl Fn(&Word) -> bool {
    move |w| {
        w.kind == "town/error"
            && w.body.part("message").and_then(|m| m.text()).is_some_and(|m| m.contains(of))
    }
}

/// a shell script in `dir`, named `name`, that is `body`: a program town may run
pub fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join(name);
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

/// whether a word is of the resident whose place is or ends in `place`, in `tense`: present, its
/// fact, as it is here; past, its `now`, as it has said each fact it has
pub fn resident(place: &str, tense: Tense) -> impl Fn(&Word) -> bool + '_ {
    move |w: &Word| {
        w.tense == tense
            && (tense != Tense::Present || !w.body.is_none())
            && w.kind
                .strip_prefix(&format!("{RESIDENTS}/"))
                .is_some_and(|k| k == place || k.ends_with(&format!("/{place}")))
    }
}

/// the probe, a Lua file in `dir` that `lua` speaks for: a future `probe` it answers as `probed`,
/// so `settled` knows all said before it
pub fn probe(dir: &Path) -> PathBuf {
    let probe = dir.join("zz-probe.lua");
    let file = r#"return react { on({ kind = "probe", tense = "future" }, function(w)
        return event("probed", w.body) end) }"#;
    std::fs::write(&probe, file).unwrap();
    probe
}

/// Every word said, but the probe's, until all is said: `lua` hears in order, so once its probe
/// answers a `probe`, it has said all it would of each word before; and once a probe answers with
/// nothing said between, nothing is still coming back. A want is not kept, so none is said until
/// the probe has come.
pub async fn settled(town: &Town, words: &mut Listening) -> Vec<Word> {
    static N: AtomicU64 = AtomicU64::new(0);
    let here = |town: &Town| {
        town.facts(&format!("{RESIDENTS}/")).keys().any(|k| k.ends_with("/zz-probe.lua"))
    };
    let mut said = Vec::new();
    while !here(town) {
        said.push(until(words, |_| true).await);
    }
    loop {
        let n = N.fetch_add(1, Ordering::Relaxed) as i64;
        town.talk(Word::new("probe", Tense::Future, Body::of(n)));
        let before = said.len();
        guard(async {
            loop {
                let w = words.recv().await.expect("the town closed");
                if w.kind == "probed" && w.body == Body::of(n) {
                    return;
                }
                if w.kind != "probe" && w.kind != "probed" {
                    said.push(w);
                }
            }
        })
        .await;
        if said.len() == before {
            return said;
        }
    }
}

/// The process's world, with `program`'s folder first on its PATH, so a town there finds it by
/// name.
pub fn world_finding(
    program: &Path,
) -> std::collections::BTreeMap<std::ffi::OsString, std::ffi::OsString> {
    let mut world: std::collections::BTreeMap<_, _> = std::env::vars_os().collect();
    let path = world.get(std::ffi::OsStr::new("PATH")).cloned().unwrap_or_default();
    let first = program.parent().map(Path::to_path_buf).into_iter();
    let path = std::env::join_paths(first.chain(std::env::split_paths(&path))).unwrap_or(path);
    world.insert("PATH".into(), path);
    world
}
