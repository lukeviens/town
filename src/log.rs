//! The fact log on disk: append a word, read the tail, compact. Town folds this into the present on
//! boot and appends every fact here — this module is the only thing that touches the file.

use std::path::Path;

use crate::word::Word;

/// Append a fact as one line, and how many bytes it took. Best-effort — logs and swallows on
/// failure.
pub fn append(path: &Path, word: &Word) -> u64 {
    let line = crate::connection::line(word) + "\n";
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

/// Read the last `bytes` of the log as whole lines (dropping a leading partial line). The byte
/// window can land mid-line AND mid-UTF-8-char, so we read RAW bytes and lossy-decode — a naive
/// `read_to_string` errors on a split char and (when the error is swallowed) yields nothing.
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
            // the first line is partial (its head byte may be a split char) — drop it
            buf.drain(..=nl);
        }
    }
    buf.lines().map(str::to_string).collect::<Vec<_>>().into_iter()
}

/// Rewrite the log down to a bounded recent tail, and before it the present facts the tail does not
/// hold, so it can't grow without end, and every fact is in it once, in the order it was said: the
/// present survives, and the tail keeps recent history for `Town::past`.
pub fn compact(path: &Path, present: &[Word], max_bytes: u64) {
    let recent: Vec<String> = tail(path, max_bytes).collect();
    let held: std::collections::HashSet<String> = recent
        .iter()
        .filter_map(|l| crate::connection::read(l).ok())
        .filter(Word::is_fact)
        .map(|w| w.kind)
        .collect();
    let head: String = present
        .iter()
        .filter(|w| !held.contains(&w.kind))
        .map(|w| crate::connection::line(w) + "\n")
        .collect();
    let recent: String = recent.into_iter().map(|l| l + "\n").collect();
    // ATOMIC: write a sibling .tmp, fsync it, then rename over the log. This file is the ONLY
    // persistent copy of the present facts; a plain truncate+write
    // would lose everything if we died between the two. rename() on the same dir is atomic.
    let mut tmp = path.to_path_buf().into_os_string();
    tmp.push(".tmp");
    let tmp = std::path::PathBuf::from(tmp);
    let write = || -> std::io::Result<()> {
        use std::io::Write;
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(head.as_bytes())?;
        f.write_all(recent.as_bytes())?;
        f.sync_all()?; // durable before the rename, so the log is never left half-written
        std::fs::rename(&tmp, path)
    };
    if let Err(e) = write() {
        eprintln!("town: past: compaction failed for {} ({e}); log left intact", path.display());
        let _ = std::fs::remove_file(&tmp); // don't strand a partial .tmp
    }
}
