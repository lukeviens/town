//! Paths: the square and the past live in ~/.cache/town; a town's programs are wherever it is told.

use std::path::PathBuf;

fn home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").expect("town needs $HOME set"))
}

/// town's own folder: the square and the log. Yours alone (700): what was said is no one else's
/// to read.
fn cache() -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    static MADE: std::sync::Once = std::sync::Once::new();
    let d = home().join(".cache/town");
    MADE.call_once(|| {
        if let Err(e) = std::fs::create_dir_all(&d) {
            eprintln!("town: cannot create {}: {e}", d.display());
        }
        std::fs::set_permissions(&d, std::fs::Permissions::from_mode(0o700)).ok();
    });
    d
}

pub fn square() -> PathBuf {
    cache().join("square.sock")
}

pub fn past() -> PathBuf {
    cache().join("past.log")
}
