//! Paths: the square and the past live in ~/.cache/town; a town's lib.lua and residents in its home.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

fn home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").expect("town needs $HOME set"))
}

/// town's own folder: the square, the log, what the surfaces keep. Yours alone (700), whatever
/// each file inside is — history and sessions are no one else's to read.
fn cache() -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let d = home().join(".cache/town");
    if let Err(e) = std::fs::create_dir_all(&d) {
        eprintln!("town: cannot create {}: {e}", d.display());
    }
    std::fs::set_permissions(&d, std::fs::Permissions::from_mode(0o700)).ok();
    d
}

pub fn square() -> PathBuf {
    cache().join("square.sock")
}

/// which program the running town is: its binary's sha256 and its pid, written as it starts
pub fn running() -> PathBuf {
    cache().join("running")
}

pub fn past() -> PathBuf {
    cache().join("past.log")
}

/// a town's home: its lib.lua and its residents. $TOWN_HOME, else ~/.config/town.
pub fn town_home() -> PathBuf {
    std::env::var_os("TOWN_HOME").map(PathBuf::from).unwrap_or_else(|| home().join(".config/town"))
}

pub fn residents() -> PathBuf {
    town_home().join("residents")
}

pub fn lib() -> PathBuf {
    town_home().join("lib.lua")
}

/// the source this VM was built from, wherever that was, so a rebuild it is missing can be seen
pub fn src() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src")
}

/// The newest mtime anywhere under `dir`.
pub fn newest(dir: &Path) -> Option<SystemTime> {
    let mut best: Option<SystemTime> = None;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            if e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                stack.push(e.path());
            } else if let Ok(m) = e.metadata().and_then(|m| m.modified()) {
                if best.map_or(true, |b| m > b) {
                    best = Some(m);
                }
            }
        }
    }
    best
}

/// Is the running core older than the source it was built from? The residents are Lua and reload
/// themselves; this binary only changes when setup.sh rebuilds it. None when there is no source
/// tree to compare against.
pub fn stale(exe: &Path, src: &Path) -> Option<bool> {
    let built = std::fs::metadata(exe).and_then(|m| m.modified()).ok()?;
    Some(newest(src)? > built)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_core_older_than_its_source_is_stale() {
        let dir = std::env::temp_dir().join(format!("town-stale-{}", std::process::id()));
        let src = dir.join("src/deep");
        std::fs::create_dir_all(&src).unwrap();
        let exe = dir.join("town");
        std::fs::write(&exe, b"binary").unwrap();
        // a source file written after the binary — found even nested, and the answer is "stale"
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(src.join("town.rs"), b"fn main() {}").unwrap();
        assert_eq!(stale(&exe, &dir.join("src")), Some(true));
        // rebuild: touch the binary, and it is current again
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(&exe, b"binary").unwrap();
        assert_eq!(stale(&exe, &dir.join("src")), Some(false));
        // nothing to compare against is not a claim of staleness
        assert_eq!(stale(&exe, &dir.join("absent")), None);
        std::fs::remove_dir_all(&dir).ok();
    }
}
