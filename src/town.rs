//! The town: the present, and the stream of words. `talk` folds a present word in,
//! keeps every word in the past, and sends it on to all who listen.

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::rc::Rc;

use serde_json::Value;
use tokio::sync::broadcast;

use crate::word::{Tense, Word};
use crate::{log, paths};

/// The lines of what a program or a connection says, as text, whatever bytes they hold: what is
/// not UTF-8 reads as �, so one bad line is a line like any other, and what comes after it is
/// heard. Safe to wait on in a `select!`: a line half read is kept for the next wait.
pub struct Lines<R>(tokio::io::BufReader<R>, Vec<u8>);

impl<R: tokio::io::AsyncRead + Unpin> Lines<R> {
    pub fn new(r: R) -> Self {
        Lines(tokio::io::BufReader::new(r), Vec::new())
    }

    /// the next line, without its end; nothing once the stream has ended or failed
    pub async fn next(&mut self) -> Option<String> {
        use tokio::io::AsyncBufReadExt;
        let more = matches!(self.0.read_until(b'\n', &mut self.1).await, Ok(n) if n > 0);
        if !more && self.1.is_empty() {
            return None;
        }
        let end = [&b"\r\n"[..], b"\n"].into_iter().find(|e| self.1.ends_with(e));
        self.1.truncate(self.1.len() - end.map_or(0, |e| e.len()));
        let line = String::from_utf8_lossy(&self.1).into_owned();
        self.1.clear();
        Some(line)
    }
}

/// A program town has joined: where its lines go, and its run.
pub type Joined = (tokio::sync::mpsc::UnboundedSender<String>, tokio::task::AbortHandle);

pub struct Town {
    /// kind → its latest present body
    pub present: RefCell<BTreeMap<String, Value>>,
    /// every word, to all who listen
    pub words: broadcast::Sender<Word>,
    /// bumped on reopen; old listeners retire
    pub age: Cell<u64>,
    /// the fact log this town folds + appends
    pub log: std::path::PathBuf,
    /// what runs for the residents (watches, joins, programs) until the town opens again
    pub tasks: RefCell<Vec<tokio::task::JoinHandle<()>>>,
    /// kind → a program town has joined: its stdin, which a future word of the kind is written to,
    /// and its run, ended when the kind is joined again
    pub joins: RefCell<std::collections::HashMap<String, Joined>>,
    /// kinds only town itself makes — `work`, `error`, what its watches and joins say, and what a
    /// resident declares its own — and how many hold each. The square refuses them from outside:
    /// whoever connects may say what they know, but not make town run something, nor put words in
    /// its machinery's mouth.
    pub inside: RefCell<std::collections::HashMap<String, usize>>,
    /// the process group of each program running, ended when the town opens again or stops
    pub groups: RefCell<std::collections::HashSet<crate::unix::Pid>>,
}

impl Town {
    pub fn new() -> Rc<Town> {
        Self::with_log(paths::past())
    }

    /// Like `new`, but folding + appending an explicit log path — the seam that lets tests run
    /// against an isolated tempfile instead of the real ~/.cache/town/past.log.
    pub fn with_log(log: std::path::PathBuf) -> Rc<Town> {
        // the present is the fold of the past — rebuild it from the log on boot
        let past = std::fs::read_to_string(&log).unwrap_or_default();
        let present = fold(past.lines().filter_map(Word::from_line));
        let (words, _) = broadcast::channel(1024);
        Rc::new(Town {
            present: RefCell::new(present),
            words,
            age: Cell::new(0),
            log,
            tasks: RefCell::new(Vec::new()),
            joins: RefCell::new(std::collections::HashMap::new()),
            inside: RefCell::new(Self::machinery()),
            groups: RefCell::new(std::collections::HashSet::new()),
        })
    }

    /// what town's own machinery says before any resident adds to it: the commands it runs, the
    /// waits it keeps, and what failed
    pub fn machinery() -> std::collections::HashMap<String, usize> {
        ["work", "after", "join", "error"].into_iter().map(|k| (k.to_string(), 1)).collect()
    }

    /// a kind only town makes, for as long as someone holds it
    pub fn hold(&self, kind: &str) {
        *self.inside.borrow_mut().entry(kind.to_string()).or_default() += 1;
    }

    /// one holder of a kind lets go; when none hold it, anyone may say it
    pub fn free(&self, kind: &str) {
        let mut inside = self.inside.borrow_mut();
        if let Some(n) = inside.get_mut(kind) {
            *n -= 1;
            if *n == 0 {
                inside.remove(kind);
            }
        }
    }

    /// Something failed: said as a past `error` word, `{ from, heard, message }`, so whoever
    /// listens can see it, and printed for the log. `heard` is the kind of word in hand when it
    /// failed, if there was one.
    pub fn error(&self, from: &str, heard: Option<&str>, message: String) {
        eprintln!("town: {from}: {message}");
        let mut body = serde_json::json!({ "from": from, "message": message });
        if let Some(kind) = heard {
            body["heard"] = kind.into();
        }
        self.talk(Word::new("error", Tense::Past, body));
    }

    /// may a word from outside be said in town? Not one only town itself makes.
    pub fn admits(&self, w: &Word) -> bool {
        !self.inside.borrow().contains_key(&w.kind)
    }

    /// A line heard at an edge, as a word: nothing if it is blank, and an `error` from `from` if
    /// it is no word. The one place bytes become a word.
    pub fn hear(&self, from: &str, line: &str) -> Option<Word> {
        if line.trim().is_empty() {
            return None;
        }
        serde_json::from_str(line)
            .map_err(|e| self.error(from, None, format!("not a word: {e}")))
            .ok()
    }

    // Talk a word: only facts fold into the present and land in the log (events and
    // intentions are transient — nothing ever reads them back); every word is sent on. A fact of
    // nothing is logged too, so the present rebuilt from the log forgets what this one did.
    pub fn talk(&self, mut word: Word) {
        if word.when == 0 {
            word.when = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0);
        }
        if word.is_fact() {
            keep(&mut self.present.borrow_mut(), &word);
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
            .map(|(k, body)| Word::new(k.clone(), Tense::Present, body.clone()))
            .collect()
    }

    /// The past of a kind: its present-tense bodies, in the order they happened, but for the
    /// times there was none. Reads only the TAIL of the log (recent history is all a caller
    /// wants) so the cost is bounded no matter how big the log grows.
    pub fn past(&self, kind: &str) -> Vec<Value> {
        log::tail(&self.log, 128 * 1024)
            .filter_map(|l| Word::from_line(&l))
            .filter(|w| w.kind == kind && w.is_fact() && !w.body.is_null())
            .map(|w| w.body)
            .collect()
    }
}

/// The present is the fold of the past: replay present words, latest of each kind wins.
/// Events (past) and intentions (future) are not facts, so they don't fold in.
pub fn fold(words: impl Iterator<Item = Word>) -> BTreeMap<String, Value> {
    let mut present = BTreeMap::new();
    for w in words {
        keep(&mut present, &w);
    }
    present
}

/// A fact sets its kind in the present. A fact of nothing, one with no body, is the fact that
/// there is none: its kind leaves the present.
fn keep(present: &mut BTreeMap<String, Value>, w: &Word) {
    if !w.is_fact() {
        return;
    }
    match w.body {
        Value::Null => present.remove(&w.kind),
        _ => present.insert(w.kind.clone(), w.body.clone()),
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn present_is_the_fold_of_the_past() {
        let past = "\
{\"kind\":\"theme\",\"tense\":\"present\",\"body\":{\"bg\":\"#111\"}}
{\"kind\":\"key\",\"tense\":\"past\",\"body\":{\"press\":\"f\"}}
{\"kind\":\"time\",\"tense\":\"future\",\"body\":{\"who\":\"me\"}}
{\"kind\":\"theme\",\"tense\":\"present\",\"body\":{\"bg\":\"#222\"}}";
        let p = fold(past.lines().filter_map(Word::from_line));
        assert_eq!(p["theme"]["bg"], "#222"); // a fact, latest wins
        assert!(!p.contains_key("key")); // an event is not a fact
        assert!(!p.contains_key("time")); // an intention is not a fact
    }

    // a line that is not UTF-8 is a line like any other, and those after it are heard; a last one
    // without its end is heard too
    #[test]
    fn lines_are_read_whatever_they_hold() {
        let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
        rt.block_on(async {
            let mut lines = Lines::new(&b"one\r\nbad \xff here\n{\"kind\":\"a\"}\nlast"[..]);
            let mut said = Vec::new();
            while let Some(line) = lines.next().await {
                said.push(line);
            }
            assert_eq!(said, ["one", "bad \u{fffd} here", "{\"kind\":\"a\"}", "last"]);
        });
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

    // A byte window that splits a multibyte char must still yield the whole tail.
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
        // a window of min_w lands `from` ON the last line's start (drops it, as intended); go one
        // past it
        let min_w = (content.len() - last_start) as u64;
        for w in (min_w + 1)..=len {
            // some of these windows land `from` inside 世/界 in the line above — the split char must
            // not panic or empty the tail; the last line ("zzzz") stays fully in the window
            let got: Vec<String> = crate::log::tail(&path, w).collect();
            assert!(got.iter().any(|l| l.contains("zzzz")), "window {w} lost the tail");
        }
        std::fs::remove_file(&path).ok();
    }

    // Compaction with a tiny tail window drops the old `notes` line from the tail, but the
    // fact must survive (present is written first) and reload byte-for-byte — and leave no .tmp.
    #[test]
    fn compaction_preserves_present_and_leaves_no_tmp() {
        let path = tmp_log();
        let town = Town::with_log(path.clone());
        town.talk(Word::new("notes", Tense::Present, serde_json::json!({ "first": "buy milk" })));
        for i in 0..50u32 {
            town.talk(Word::new(
                "theme",
                Tense::Present,
                serde_json::json!({ "bg": format!("#{i:06x}") }),
            ));
        }
        let before = town.present.borrow().clone();
        town.compact(512); // tiny → notes falls out of the recent tail
        let re = Town::with_log(path.clone()); // reload from the compacted log
        assert_eq!(*re.present.borrow(), before, "present not preserved across compaction");
        assert_eq!(re.present.borrow()["notes"]["first"], "buy milk");
        let mut tmp = path.clone().into_os_string();
        tmp.push(".tmp");
        assert!(!std::path::Path::new(&tmp).exists(), "left a .tmp behind");
        std::fs::remove_file(&path).ok();
    }

    // a fact of nothing takes its kind out of the present, here and in a town rebuilt from the
    // log, after compaction too; and `past` passes over it
    #[test]
    fn a_fact_of_nothing_forgets() {
        let path = tmp_log();
        let town = Town::with_log(path.clone());
        town.talk(Word::new("song", Tense::Present, serde_json::json!({ "title": "a" })));
        town.talk(Word::new("theme", Tense::Present, serde_json::json!({ "bg": "#111" })));
        town.talk(Word::new("song", Tense::Present, Value::Null));
        assert!(!town.present.borrow().contains_key("song"));
        assert!(town.present.borrow().contains_key("theme"));
        assert!(!Town::with_log(path.clone()).present.borrow().contains_key("song"));
        town.compact(64 * 1024);
        assert!(!Town::with_log(path.clone()).present.borrow().contains_key("song"));
        assert_eq!(town.past("song"), [serde_json::json!({ "title": "a" })]);
        std::fs::remove_file(&path).ok();
    }

    // a kind held twice stays inside until both let go
    #[test]
    fn a_kind_is_inside_while_any_hold_it() {
        let town = Town::with_log(tmp_log());
        let mine = Word::new("mine", Tense::Past, Value::Null);
        town.hold("mine");
        town.hold("mine");
        town.free("mine");
        assert!(!town.admits(&mine), "one still holds it");
        town.free("mine");
        assert!(town.admits(&mine), "none do");
    }

    // talk folds facts into present + appends exactly one log line; events/intentions do neither.
    #[test]
    fn talk_folds_facts_only() {
        let path = tmp_log();
        let town = Town::with_log(path.clone());
        town.talk(Word::new("theme", Tense::Present, serde_json::json!({"bg":"#111"})));
        town.talk(Word::new("key", Tense::Past, serde_json::json!({"press":"f"})));
        town.talk(Word::new("time", Tense::Future, serde_json::json!({"who":"me"})));
        assert_eq!(town.present.borrow()["theme"]["bg"], "#111");
        assert!(!town.present.borrow().contains_key("key"));
        assert!(!town.present.borrow().contains_key("time"));
        assert_eq!(std::fs::read_to_string(&path).unwrap().lines().count(), 1);
        std::fs::remove_file(&path).ok();
    }
}
