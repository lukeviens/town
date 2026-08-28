//! town as a library — the same modules the binary runs, exposed so tests (and any other
//! crate) can reach the pure core (`word`, `town::fold`) and the engine directly.

pub mod word;
pub mod town;
pub mod residents;
pub mod square;
pub mod paths;
