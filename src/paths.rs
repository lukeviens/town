//! Paths: the square and the past live in ~/.cache/town; the residents in the dotfiles.

use std::path::PathBuf;

fn home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").expect("town needs $HOME set"))
}

fn cache() -> PathBuf {
    let d = home().join(".cache/town");
    if let Err(e) = std::fs::create_dir_all(&d) {
        eprintln!("town: cannot create {}: {e}", d.display());
    }
    d
}

pub fn square() -> PathBuf {
    cache().join("square.sock")
}

pub fn past() -> PathBuf {
    cache().join("past.log")
}

pub fn residents() -> PathBuf {
    home().join(".config/town/residents")
}

pub fn lib() -> PathBuf {
    home().join(".config/town/lib.lua")
}
