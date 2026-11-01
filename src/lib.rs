//! town as a library — the same modules the binary runs, exposed so tests (and any other
//! crate) can reach the pure core (`word`, `town::fold`) and the engine directly.

pub mod word;
pub mod town;
pub mod log;
pub mod residents;
pub mod square;
pub mod paths;
pub mod work;

/// How a resident is written: the tenses, `on`/`react`, `work`, the file helpers. Every town's
/// residents are loaded after it, and after the town's own lib.lua.
pub const RESIDENT: &str = include_str!("resident.lua");
