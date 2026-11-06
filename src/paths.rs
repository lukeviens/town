//! Town's home: where it keeps the square and the log.
//!
//! - **as it is**: `~/.town`.
//! - **another**: `TOWN_HOME`. Set it, too, for every program that reaches the square, or it finds
//!   no town there.

use std::path::PathBuf;

fn home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").expect("town needs $HOME set"))
}

/// town's own folder, yours alone (700): what was said is no one else's to read.
fn town_home() -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    static MADE: std::sync::Once = std::sync::Once::new();
    let d = std::env::var_os("TOWN_HOME").map_or_else(|| home().join(".town"), PathBuf::from);
    MADE.call_once(|| {
        if let Err(e) = std::fs::create_dir_all(&d) {
            eprintln!("town: cannot create {}: {e}", d.display());
        }
        std::fs::set_permissions(&d, std::fs::Permissions::from_mode(0o700)).ok();
    });
    d
}

/// What one town keeps while it runs, so no other opens on its log.
pub fn lock() -> PathBuf {
    town_home().join("town.lock")
}

pub fn square() -> PathBuf {
    town_home().join("square.sock")
}

pub fn past() -> PathBuf {
    town_home().join("past.log")
}
