//! town as a library: the modules the binary runs, for tests and any other crate.

pub mod body;
pub mod language;
pub mod log;
pub mod parts;
pub mod paths;
pub mod program;
pub mod residents;
pub mod square;
pub mod town;
pub mod unix;
pub mod voice;
pub mod word;
pub mod work;

#[cfg(any(test, feature = "testing"))]
pub mod testing;
