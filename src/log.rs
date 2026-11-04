//! The log: each fact, on disk, and all that touches it. The README's *words*, the log.
//!
//! - **as town starts**: folded into the present.
//! - **as a fact is said**: appended, and the log compacted as it grows.

use std::path::Path;

use crate::word::Word;

/// Append a fact as one line, and how many bytes it took: none if it could not, which is said.
pub fn append(path: &Path, word: &Word) -> u64 {
    let line = crate::voice::line(word) + "\n";
    match std::fs::OpenOptions::new().create(true).append(true).open(path) {
        Ok(mut past) => {
            use std::io::Write;
            if past.write_all(line.as_bytes()).is_err() {
                eprintln!("town: past: append failed for '{}'", word.kind);
                return 0;
            }
            line.len() as u64
        }
        Err(e) => {
            eprintln!("town: past: cannot open {}: {e}", path.display());
            0
        }
    }
}

/// The whole log, line by line. A line that is not text, as one cut short, is read as best it can
/// be: it is no word, and the lines around it stand.
pub fn all(path: &Path) -> impl Iterator<Item = String> {
    tail(path, u64::MAX)
}

/// The last `bytes` of the log, as whole lines. The window may begin inside a line, or inside a
/// char, so its first line is left out, and its bytes are read as best they can be, never refused.
pub fn tail(path: &Path, bytes: u64) -> impl Iterator<Item = String> {
    use std::io::{Read, Seek, SeekFrom};
    let mut raw = Vec::new();
    let mut from = 0;
    if let Ok(mut f) = std::fs::File::open(path) {
        let len = f.metadata().map(|m| m.len()).unwrap_or(0);
        from = len.saturating_sub(bytes);
        let _ = f.seek(SeekFrom::Start(from));
        let _ = f.read_to_end(&mut raw);
    }
    let mut buf = String::from_utf8_lossy(&raw).into_owned();
    if from > 0 {
        if let Some(nl) = buf.find('\n') {
            // the first line is a part of one
            buf.drain(..=nl);
        }
    }
    buf.lines().map(str::to_string).collect::<Vec<_>>().into_iter()
}

/// Rewrite the log as its last `max_bytes`, after the present facts those lack: it stays bounded,
/// holds the present, and keeps the recent past for `Town::past`.
pub fn compact(path: &Path, present: &[Word], max_bytes: u64) {
    let recent: Vec<String> = tail(path, max_bytes).collect();
    let held: std::collections::HashSet<String> = recent
        .iter()
        .filter_map(|l| crate::voice::read(l).ok())
        .filter(Word::is_fact)
        .map(|w| w.kind)
        .collect();
    let head: String = present
        .iter()
        .filter(|w| !held.contains(&w.kind))
        .map(|w| crate::voice::line(w) + "\n")
        .collect();
    let recent: String = recent.into_iter().map(|l| l + "\n").collect();
    // the log is the one copy of the present, and town may stop at any moment: the new log is
    // written beside it, then renamed over it at once
    let mut tmp = path.to_path_buf().into_os_string();
    tmp.push(".tmp");
    let tmp = std::path::PathBuf::from(tmp);
    let write = || -> std::io::Result<()> {
        use std::io::Write;
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(head.as_bytes())?;
        f.write_all(recent.as_bytes())?;
        f.sync_all()?; // on the disk before the rename, so the log is never half written
        std::fs::rename(&tmp, path)
    };
    if let Err(e) = write() {
        eprintln!("town: past: compaction failed for {} ({e}); log left intact", path.display());
        let _ = std::fs::remove_file(&tmp); // no part of a log is left beside it
    }
}
