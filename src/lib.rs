//! town as a library — the same modules the binary runs, so tests, and any other crate, can reach
//! the word, the present's fold, and the rest of town directly.

pub mod body;
pub mod connection;
pub mod language;
pub mod log;
pub mod lua;
pub mod parts;
pub mod paths;
pub mod residents;
pub mod square;
pub mod town;
pub mod unix;
pub mod word;
pub mod work;

#[cfg(test)]
mod testing;

/// How a resident is written: the tenses, `on`/`react`, `work`, the file helpers. A town's own
/// files are all loaded after it.
pub const RESIDENT: &str = include_str!("resident.lua");
