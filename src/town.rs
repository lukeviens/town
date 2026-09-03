//! The town: the present, and the stream of words. `talk` folds a present word in,
//! keeps every word in the past, and sends it on to all who listen.

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::rc::Rc;

use serde_json::Value;
use tokio::sync::broadcast;

use crate::word::{Tense, Word};
use crate::{log, paths};

pub struct Town {
    pub present: RefCell<BTreeMap<String, Value>>, // kind → its latest present body
    pub words: broadcast::Sender<Word>,            // every word, to all who listen
    pub age: Cell<u64>,                            // bumped on reopen; old listeners retire
    pub log: std::path::PathBuf,                   // the fact log this town folds + appends
}

impl Town {
    pub fn new() -> Rc<Town> {
        Self::at(paths::past())
    }

    /// Like `new`, but folding + appending an explicit log path — the seam that lets tests run
    /// against an isolated tempfile instead of the real ~/.cache/town/past.log.
    pub fn at(log: std::path::PathBuf) -> Rc<Town> {
        // the present is the fold of the past — rebuild it from the log on boot
        let past = std::fs::read_to_string(&log).unwrap_or_default();
        let present = fold(past.lines().filter_map(Word::from_line));
        let (words, _) = broadcast::channel(1024);
        Rc::new(Town {
            present: RefCell::new(present),
            words,
            age: Cell::new(0),
            log,
        })
    }

    // Talk a word: only facts fold into the present and land in the log (events and
    // intentions are transient — nothing ever reads them back); every word is sent on.
    pub fn talk(&self, word: Word) {
        if word.is_fact() {
            self.present
                .borrow_mut()
                .insert(word.kind.clone(), word.body.clone());
            log::append(&self.log, &word);
        }
        let _ = self.words.send(word);
    }

    /// Rewrite the log down to the present facts + a bounded recent tail (called on boot). See
    /// `log::compact`.
    pub fn compact(&self, max_bytes: u64) {
        log::compact(&self.log, &self.present_words(), max_bytes);
    }

    /// The present as words, to greet a new listener.
    pub fn present_words(&self) -> Vec<Word> {
        self.present
            .borrow()
            .iter()
            .map(|(k, body)| Word {
                kind: k.clone(),
                tense: Tense::Present,
                body: body.clone(),
            })
            .collect()
    }

    /// The past of a kind: its present-tense bodies, in the order they happened. Reads only
    /// the TAIL of the log (recent history is all a caller wants — the picker's recent list)
    /// so the cost is bounded no matter how big the log grows.
    pub fn past(&self, kind: &str) -> Vec<Value> {
        log::tail(&self.log, 128 * 1024)
            .filter_map(|l| Word::from_line(&l))
            .filter(|w| w.kind == kind && w.is_fact())
            .map(|w| w.body)
            .collect()
    }
}

/// The present is the fold of the past: replay present words, latest of each kind wins.
/// Events (past) and intentions (future) are not facts, so they don't fold in.
pub fn fold(words: impl Iterator<Item = Word>) -> BTreeMap<String, Value> {
    let mut present = BTreeMap::new();
    for w in words {
        if w.is_fact() {
            present.insert(w.kind, w.body);
        }
    }
    present
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn present_is_the_fold_of_the_past() {
        let past = "\
{\"kind\":\"theme\",\"tense\":\"present\",\"body\":{\"bg\":\"#111\"}}
{\"kind\":\"key\",\"tense\":\"past\",\"body\":{\"press\":\"f\"}}
{\"kind\":\"pick\",\"tense\":\"future\",\"body\":{\"what\":\"apps\"}}
{\"kind\":\"theme\",\"tense\":\"present\",\"body\":{\"bg\":\"#222\"}}";
        let p = fold(past.lines().filter_map(Word::from_line));
        assert_eq!(p["theme"]["bg"], "#222"); // a fact, latest wins
        assert!(!p.contains_key("key")); // an event is not a fact
        assert!(!p.contains_key("pick")); // an intention is not a fact
    }

    fn tmp_log() -> std::path::PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        std::env::temp_dir().join(format!(
            "town-test-{}-{}.log",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ))
    }

    // Regression for the tail_lines UTF-8 bug: a byte window that splits a multibyte char must
    // still yield the whole tail (the old read_to_string returned empty and it was swallowed).
    #[test]
    fn tail_survives_a_utf8_split_at_the_window_edge() {
        let path = tmp_log();
        let content = "\
{\"kind\":\"a\",\"tense\":\"present\",\"body\":{\"x\":\"aaaaaaaa\"}}
{\"kind\":\"b\",\"tense\":\"present\",\"body\":{\"x\":\"héllo-世界\"}}
{\"kind\":\"c\",\"tense\":\"present\",\"body\":{\"x\":\"zzzz\"}}\n";
        std::fs::write(&path, content).unwrap();
        let len = std::fs::metadata(&path).unwrap().len();
        let last_start = content.rfind("{\"kind\":\"c\"").unwrap();
        let min_w = (content.len() - last_start) as u64; // window == this lands `from` ON the last line's
        for w in (min_w + 1)..=len {                      // start (drops it, as intended); go one past it
            // some of these windows land `from` inside 世/界 in the line above — the split char must
            // not panic or empty the tail; the last line ("zzzz") stays fully in the window
            let got: Vec<String> = crate::log::tail(&path, w).collect();
            assert!(got.iter().any(|l| l.contains("zzzz")), "window {w} lost the tail");
        }
        std::fs::remove_file(&path).ok();
    }

    // Compaction with a tiny tail window drops the old `favourites` line from the tail, but the
    // fact must survive (present is written first) and reload byte-for-byte — and leave no .tmp.
    #[test]
    fn compaction_preserves_present_and_leaves_no_tmp() {
        let path = tmp_log();
        let town = Town::at(path.clone());
        town.talk(Word {
            kind: "favourites".into(),
            tense: Tense::Present,
            body: serde_json::json!({ "1": { "app": "X" } }),
        });
        for i in 0..50u32 {
            town.talk(Word {
                kind: "theme".into(),
                tense: Tense::Present,
                body: serde_json::json!({ "bg": format!("#{i:06x}") }),
            });
        }
        let before = town.present.borrow().clone();
        town.compact(512); // tiny → favourites falls out of the recent tail
        let re = Town::at(path.clone()); // reload from the compacted log
        assert_eq!(*re.present.borrow(), before, "present not preserved across compaction");
        assert_eq!(re.present.borrow()["favourites"]["1"]["app"], "X");
        let mut tmp = path.clone().into_os_string();
        tmp.push(".tmp");
        assert!(!std::path::Path::new(&tmp).exists(), "left a .tmp behind");
        std::fs::remove_file(&path).ok();
    }

    // talk folds facts into present + appends exactly one log line; events/intentions do neither.
    #[test]
    fn talk_folds_facts_only() {
        let path = tmp_log();
        let town = Town::at(path.clone());
        town.talk(Word { kind: "theme".into(), tense: Tense::Present, body: serde_json::json!({"bg":"#111"}) });
        town.talk(Word { kind: "key".into(), tense: Tense::Past, body: serde_json::json!({"press":"f"}) });
        town.talk(Word { kind: "pick".into(), tense: Tense::Future, body: serde_json::json!({"what":"apps"}) });
        assert_eq!(town.present.borrow()["theme"]["bg"], "#111");
        assert!(!town.present.borrow().contains_key("key"));
        assert!(!town.present.borrow().contains_key("pick"));
        assert_eq!(std::fs::read_to_string(&path).unwrap().lines().count(), 1);
        std::fs::remove_file(&path).ok();
    }
}
