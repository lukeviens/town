//! What tests share, within town and beside it.
//!
//! - **a runtime**, and **a folder** of their own.
//! - **waiting** for a word, never for a span of time, under a guard, so a test that would hang
//!   fails.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use crate::town::Listening;
use crate::word::Word;

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

/// what `f` comes to, guarded
pub async fn guard<T>(f: impl Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(30), f).await.expect("never came")
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

/// a `town/error` whose message holds `of`
pub fn an_error(of: &'static str) -> impl Fn(&Word) -> bool {
    move |w| {
        w.kind == "town/error"
            && w.body.part("message").and_then(|m| m.text()).is_some_and(|m| m.contains(of))
    }
}

/// The process's world, with the folder of `program`, one of town's own built beside it, first on
/// its PATH, as `town` puts its own there.
pub fn world_with(
    program: &Path,
) -> std::collections::BTreeMap<std::ffi::OsString, std::ffi::OsString> {
    let mut world: std::collections::BTreeMap<_, _> = std::env::vars_os().collect();
    let beside = program.parent().map(Path::to_path_buf).unwrap_or_default();
    let path = std::env::var_os("PATH").unwrap_or_default();
    let paths = [beside].into_iter().chain(std::env::split_paths(&path));
    world.insert("PATH".into(), std::env::join_paths(paths).unwrap_or_default());
    world
}
