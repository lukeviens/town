//! The log: each fact, kept, and all that touches it. The README's *town*, the log.
//!
//! - **as town starts**: folded into the present.
//! - **as a fact is said**: appended; and the log compacted as a kind's owner keeps less of it.
//! - **where**: in a store, which keeps its bytes in order: a file, as town's own is, or memory.
//! - **in**: town's own language, as `voice::own` writes a word and `voice::owns` reads them.

use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;

use crate::word::Word;

/// Where a log is kept: its bytes, in the order said. Town asks no more of it, and it knows nothing
/// of words: a file is one, memory another.
pub trait Store {
    /// `bytes` added at the end
    fn append(&self, bytes: &[u8]) -> std::io::Result<()>;
    /// all it contains; none, if it contains nothing yet; or why it cannot be read
    fn read(&self) -> std::io::Result<Vec<u8>>;
    /// all it contains made `all`, at once: one who reads it after reads the old or the new, whole
    fn replace(&self, all: &[u8]) -> std::io::Result<()>;
}

/// A file: made as the first bytes are appended.
impl Store for PathBuf {
    fn append(&self, bytes: &[u8]) -> std::io::Result<()> {
        use std::io::Write;
        let at =
            |e: std::io::Error| std::io::Error::new(e.kind(), format!("{}: {e}", self.display()));
        let mut past =
            std::fs::OpenOptions::new().create(true).append(true).open(self).map_err(at)?;
        past.write_all(bytes).map_err(at)
    }

    // a file not made yet contains nothing; any other failure is no log, not an empty one
    fn read(&self) -> std::io::Result<Vec<u8>> {
        match std::fs::read(self) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
            read => read,
        }
    }

    // the log is the one copy of the present, and town may stop at any moment: the new log is
    // written beside it, then renamed over it at once
    fn replace(&self, all: &[u8]) -> std::io::Result<()> {
        use std::io::Write;
        let mut tmp = self.clone().into_os_string();
        tmp.push(".tmp");
        let tmp = PathBuf::from(tmp);
        let write = || -> std::io::Result<()> {
            let mut f = std::fs::File::create(&tmp)?;
            f.write_all(all)?;
            f.sync_all()?; // on the disk before the rename, so the log is never half written
            std::fs::rename(&tmp, self)
        };
        write().inspect_err(|_| {
            let _ = std::fs::remove_file(&tmp); // no part of a log is left beside it
        })
    }
}

/// Memory: a log kept while it is held, and gone with it.
#[cfg(any(test, feature = "testing"))]
#[derive(Default)]
pub struct Memory(std::cell::RefCell<Vec<u8>>);

#[cfg(any(test, feature = "testing"))]
impl Store for Memory {
    fn append(&self, bytes: &[u8]) -> std::io::Result<()> {
        self.0.borrow_mut().extend_from_slice(bytes);
        Ok(())
    }

    fn read(&self) -> std::io::Result<Vec<u8>> {
        Ok(self.0.borrow().clone())
    }

    fn replace(&self, all: &[u8]) -> std::io::Result<()> {
        *self.0.borrow_mut() = all.to_vec();
        Ok(())
    }
}

/// A store held by more than one: a town, and one that opens the same log again.
impl<S: Store> Store for Rc<S> {
    fn append(&self, bytes: &[u8]) -> std::io::Result<()> {
        (**self).append(bytes)
    }

    fn read(&self) -> std::io::Result<Vec<u8>> {
        (**self).read()
    }

    fn replace(&self, all: &[u8]) -> std::io::Result<()> {
        (**self).replace(all)
    }
}

/// Append a fact, whole, and whether it could be: if not, that is said.
pub fn append(store: &dyn Store, word: &Word) -> bool {
    match store.append(&crate::voice::own(word)) {
        Ok(()) => true,
        Err(e) => {
            eprintln!("town: past: append failed for '{}': {e}", word.kind);
            false
        }
    }
}

/// The words the log contains, in order, or why it cannot be read. What is no word is passed over,
/// and the words around it stand.
pub fn all(store: &dyn Store) -> std::io::Result<Vec<Word>> {
    Ok(crate::voice::owns(&store.read()?).into_iter().filter_map(|(_, w)| w).collect())
}

/// The log as a town opens it: its words, once what it ends in that is no word, as a fact cut short
/// as town stopped, is let go, so the next fact is not said after it as one. Why it cannot be read.
pub fn open(store: &dyn Store) -> std::io::Result<Vec<Word>> {
    let raw = store.read()?;
    let units = crate::voice::owns(&raw);
    match units.last() {
        Some((cut, None)) => store.replace(&raw[..raw.len() - cut.len()])?,
        // a word whole but for its line's end is ended, so the next is not said after it as one
        Some((last, Some(_))) if !last.ends_with(b"\n") => store.append(b"\n")?,
        _ => {}
    }
    Ok(units.into_iter().filter_map(|(_, w)| w).collect())
}

/// Rewrite the log as what `keeps` keeps: of each kind it names a number for, its last facts, that
/// many and one, the present among them; of each other, all; in the order they were said, and no
/// unit that is no word. How many facts of each kind it keeps then.
pub fn compact(
    store: &dyn Store,
    keeps: impl Fn(&str) -> Option<u64>,
) -> std::io::Result<HashMap<String, u64>> {
    // one that cannot be read is left as it is: none is written over what is not known
    let raw = store.read()?;
    let words: Vec<(&[u8], Word)> =
        crate::voice::owns(&raw).into_iter().filter_map(|(u, w)| Some((u, w?))).collect();
    let mut left: HashMap<String, u64> = HashMap::new();
    let mut kept = vec![false; words.len()];
    for (i, (_, w)) in words.iter().enumerate().rev() {
        let n = left.entry(w.kind.clone()).or_insert(0);
        if keeps(&w.kind).is_none_or(|keep| *n <= keep) {
            *n += 1;
            kept[i] = true;
        }
    }
    let all: Vec<u8> = words
        .iter()
        .zip(kept)
        .filter(|(_, k)| *k)
        .flat_map(|((u, _), _)| u.iter())
        .copied()
        .collect();
    store.replace(&all)?;
    Ok(left)
}
