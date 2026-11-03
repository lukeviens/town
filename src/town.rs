//! The town: the present, and the stream of words. `talk` folds a present word in,
//! keeps every word in the past, and sends it on to all who listen.

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::rc::Rc;

use tokio::sync::broadcast;

use crate::body::Body;
use crate::parts::Parts;
use crate::word::{Tense, Word};
use crate::{log, paths};

/// A program town has joined, or a socket it has connected to: where what it is told goes, its
/// run, and the language it speaks, if it speaks one, by name.
pub struct Joined {
    pub to: tokio::sync::mpsc::UnboundedSender<Vec<u8>>,
    pub run: tokio::task::AbortHandle,
    pub speaks: Option<String>,
}

/// How much of the past the log keeps after the present: all `past()` reads.
const KEEP: u64 = 128 * 1024;
/// How much the log may grow before it is compacted again.
const GROWTH: u64 = 1024 * 1024;

pub struct Town {
    /// kind → its latest present body
    pub present: RefCell<BTreeMap<String, Body>>,
    /// every word, to all who listen
    pub words: broadcast::Sender<Word>,
    /// how many times the town has opened: what listened before it last did stops
    pub age: Cell<u64>,
    /// the fact log this town folds + appends
    pub log: std::path::PathBuf,
    /// what has been appended to the log since it was last compacted
    grown: Cell<u64>,
    /// what runs for the residents (watches, joins, programs) until the town opens again
    pub tasks: RefCell<Vec<tokio::task::JoinHandle<()>>>,
    /// kind → what town has joined or connected to as that kind: told each future word of the
    /// kind, and ended when the kind is joined again
    pub joins: RefCell<std::collections::HashMap<String, Joined>>,
    /// kinds only town itself makes — its machinery, what its watches and joins say, and what a
    /// resident declares its own — and how many hold each. The square refuses them from outside:
    /// whoever connects may say what they know, but not make town run something, nor put words in
    /// its machinery's mouth.
    pub inside: RefCell<std::collections::HashMap<String, usize>>,
    /// the process group of each program running, ended when the town opens again or stops
    pub groups: RefCell<std::collections::HashSet<crate::unix::Pid>>,
    /// how bytes are read as parts, and parts written as bytes, by name
    pub languages: crate::language::Languages,
}

impl Town {
    pub fn new() -> Rc<Town> {
        Self::with_log(paths::past())
    }

    /// Like `new`, but folding and appending the log at `log`, so a test keeps its own, not
    /// ~/.cache/town/past.log.
    pub fn with_log(log: std::path::PathBuf) -> Rc<Town> {
        // the present is the fold of the past — rebuild it from the log on boot
        let past = std::fs::read_to_string(&log).unwrap_or_default();
        let present = fold(past.lines().filter_map(|l| crate::connection::read(l).ok()));
        let (words, _) = broadcast::channel(1024);
        Rc::new(Town {
            present: RefCell::new(present),
            words,
            age: Cell::new(0),
            log,
            grown: Cell::new(0),
            tasks: RefCell::new(Vec::new()),
            joins: RefCell::new(std::collections::HashMap::new()),
            inside: RefCell::new(Self::machinery()),
            groups: RefCell::new(std::collections::HashSet::new()),
            languages: Default::default(),
        })
    }

    /// what town's own machinery says before any resident adds to it: the commands it runs, the
    /// waits it keeps, and what failed
    pub fn machinery() -> std::collections::HashMap<String, usize> {
        ["work", "after", "join", "connect", "language", "error"]
            .into_iter()
            .map(|k| (k.to_string(), 1))
            .collect()
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
        self.failed(from, message, heard.map(|kind| ("heard", kind)));
    }

    /// Something said that is no word: an `error`, with what was `said`, so whoever said it can
    /// know it for theirs.
    pub fn misheard(&self, from: &str, said: &str, message: String) {
        self.failed(from, message, Some(("said", said)));
    }

    fn failed(&self, from: &str, message: String, also: Option<(&str, &str)>) {
        eprintln!("town: {from}: {message}");
        let mut body = Parts::map([("from", from), ("message", &message)]);
        if let Some((name, part)) = also {
            body.set(name, part);
        }
        self.talk(Word::new("error", Tense::Past, Body::of(body)));
    }

    /// may a word from outside be said in town? Not one only town itself makes.
    pub fn admits(&self, w: &Word) -> bool {
        !self.inside.borrow().contains_key(&w.kind)
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
        // a language said is spoken from now: as it is said, before anyone hears that it was
        if word.kind == "language" && word.tense == Tense::Past {
            self.learn(&word.body);
        }
        if word.is_fact() {
            keep(&mut self.present.borrow_mut(), &word);
            self.grown.set(self.grown.get() + log::append(&self.log, &word));
            if self.grown.get() > GROWTH {
                self.compact();
            }
        }
        let _ = self.words.send(word);
    }

    /// The language `name`, once it is said: now, if it has been, or else when it is, saying once,
    /// as `from`, that it waits for it, since a program says its language as it runs, after the
    /// files beside it have opened. Nothing if the town stops first.
    pub async fn language(&self, name: &str, from: &str) -> Option<Rc<crate::language::Language>> {
        let mut words = self.words.subscribe();
        let mut told = false;
        loop {
            if let Ok(l) = self.languages.get(name) {
                return Some(l);
            }
            if !std::mem::replace(&mut told, true) {
                self.error(
                    from,
                    None,
                    format!("no language is called {name} yet: it waits for one"),
                );
            }
            if let Err(broadcast::error::RecvError::Closed) = words.recv().await {
                return None;
            }
        }
    }

    /// A language said: `{ name, grammar }`. A later one of the same name takes its place, and
    /// the town opening again forgets them all, so its programs say theirs again.
    fn learn(&self, body: &Body) {
        let name = body.part("name").and_then(Parts::text);
        let grammar = body.part("grammar").ok_or("a language has a grammar".to_string());
        match (name, grammar.and_then(crate::language::Grammar::of)) {
            (Some("json"), _) => {
                self.error("language", Some("language"), "json is town's own".into())
            }
            (Some(name), Ok(grammar)) => self.languages.say(name, grammar),
            (None, _) => self.error("language", Some("language"), "a language has a name".into()),
            (Some(name), Err(e)) => {
                self.error("language", Some("language"), format!("{name}: {e}"))
            }
        }
    }

    /// Rewrite the log down to the present facts + a bounded recent tail: as town starts, and
    /// whenever it has grown by `GROWTH` since. See `log::compact`.
    pub fn compact(&self) {
        log::compact(&self.log, &self.present_words(), KEEP);
        self.grown.set(0);
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
    pub fn past(&self, kind: &str) -> Vec<Body> {
        log::tail(&self.log, KEEP)
            .filter_map(|l| crate::connection::read(&l).ok())
            .filter(|w| w.kind == kind && w.is_fact() && !w.body.is_none())
            .map(|w| w.body)
            .collect()
    }
}

/// The present is the fold of the past: replay present words, latest of each kind wins.
/// Events (past) and intentions (future) are not facts, so they don't fold in.
pub fn fold(words: impl Iterator<Item = Word>) -> BTreeMap<String, Body> {
    let mut present = BTreeMap::new();
    for w in words {
        keep(&mut present, &w);
    }
    present
}

/// A fact sets its kind in the present. A fact of nothing, one with no body, is the fact that
/// there is none: its kind leaves the present.
fn keep(present: &mut BTreeMap<String, Body>, w: &Word) {
    if !w.is_fact() {
        return;
    }
    match w.body.is_none() {
        true => present.remove(&w.kind),
        false => present.insert(w.kind.clone(), w.body.clone()),
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
        let p = fold(past.lines().filter_map(|l| crate::connection::read(l).ok()));
        assert_eq!(p["theme"].value()["bg"], "#222"); // a fact, latest wins
        assert!(!p.contains_key("key")); // an event is not a fact
        assert!(!p.contains_key("time")); // an intention is not a fact
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
        town.talk(Word::new(
            "notes",
            Tense::Present,
            Body::of(serde_json::json!({ "first": "buy milk" })),
        ));
        for i in 0..50u32 {
            town.talk(Word::new(
                "theme",
                Tense::Present,
                Body::of(serde_json::json!({ "bg": format!("#{i:06x}") })),
            ));
        }
        let before = town.present.borrow().clone();
        log::compact(&town.log, &town.present_words(), 512); // tiny → notes falls out of the tail
        let re = Town::with_log(path.clone()); // reload from the compacted log
        assert_eq!(*re.present.borrow(), before, "present not preserved across compaction");
        assert_eq!(re.present.borrow()["notes"].value()["first"], "buy milk");
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
        town.talk(Word::new("song", Tense::Present, Body::of(serde_json::json!({ "title": "a" }))));
        town.talk(Word::new(
            "theme",
            Tense::Present,
            Body::of(serde_json::json!({ "bg": "#111" })),
        ));
        town.talk(Word::new("song", Tense::Present, Body::none()));
        assert!(!town.present.borrow().contains_key("song"));
        assert!(town.present.borrow().contains_key("theme"));
        assert!(!Town::with_log(path.clone()).present.borrow().contains_key("song"));
        town.compact();
        assert!(!Town::with_log(path.clone()).present.borrow().contains_key("song"));
        assert_eq!(town.past("song"), [Parts::from(serde_json::json!({ "title": "a" }))]);
        std::fs::remove_file(&path).ok();
    }

    // a kind held twice stays inside until both let go
    #[test]
    fn a_kind_is_inside_while_any_hold_it() {
        let town = Town::with_log(tmp_log());
        let mine = Word::new("mine", Tense::Past, Body::none());
        town.hold("mine");
        town.hold("mine");
        town.free("mine");
        assert!(!town.admits(&mine), "one still holds it");
        town.free("mine");
        assert!(town.admits(&mine), "none do");
    }

    // the log is compacted as it grows, so it stays bounded however long town runs, and keeps
    // the present whole
    #[test]
    fn the_log_stays_bounded() {
        let path = tmp_log();
        let town = Town::with_log(path.clone());
        let line = "x".repeat(1000);
        for i in 0..3000 {
            town.talk(Word::new(format!("k{}", i % 50), Tense::Present, Body::of(line.as_str())));
        }
        assert!(std::fs::metadata(&path).unwrap().len() <= KEEP + GROWTH + 64 * 1024);
        assert_eq!(Town::with_log(path.clone()).present.borrow().len(), 50);
        std::fs::remove_file(&path).ok();
    }

    // compacted, the log holds each fact once, in the order it was said: the past is as it was
    #[test]
    fn compaction_keeps_the_past_in_order() {
        let path = tmp_log();
        let town = Town::with_log(path.clone());
        let say = |kind: &str, n: i64| town.talk(Word::new(kind, Tense::Present, Body::of(n)));
        say("old", 1);
        (0..3).for_each(|n| say("song", n));
        town.compact();
        assert_eq!(town.past("song"), [Parts::from(0i64), 1i64.into(), 2i64.into()]);
        assert_eq!(town.past("old"), [Parts::from(1i64)]);
        log::compact(&town.log, &town.present_words(), 60); // the tail holds only the last
        assert_eq!(town.past("song"), [Parts::from(2i64)]);
        assert_eq!(Town::with_log(path.clone()).present.borrow().len(), 2, "the present kept");
        std::fs::remove_file(&path).ok();
    }

    // a language said is spoken at once, before anyone hears it was said; a later one of the
    // name takes its place; one that is no grammar is an error; and only town's own say one
    #[test]
    fn a_language_is_a_word() {
        let town = Town::with_log(tmp_log());
        let mut words = town.words.subscribe();
        let say = |grammar: serde_json::Value| {
            let body = Body::of(serde_json::json!({ "name": "line", "grammar": grammar }));
            town.talk(Word::new("language", Tense::Past, body));
        };
        say(serde_json::json!({ "upto": "\n" }));
        assert_eq!(town.languages.parts("line", b"hi\n"), Ok(Parts::from("hi")));
        say(serde_json::json!({ "but": "\n" }));
        assert!(town.languages.parts("line", b"hi\n").is_err(), "the later one speaks");
        say(serde_json::json!({ "nope": "\n" }));
        let said: Vec<_> = std::iter::from_fn(|| words.try_recv().ok()).map(|w| w.kind).collect();
        assert_eq!(said, ["language", "language", "error", "language"]);
        assert!(!town.admits(&Word::new("language", Tense::Past, Body::none())));
    }

    // talk folds facts into present + appends exactly one log line; events/intentions do neither.
    #[test]
    fn talk_folds_facts_only() {
        let path = tmp_log();
        let town = Town::with_log(path.clone());
        town.talk(Word::new("theme", Tense::Present, Body::of(serde_json::json!({"bg":"#111"}))));
        town.talk(Word::new("key", Tense::Past, Body::of(serde_json::json!({"press":"f"}))));
        town.talk(Word::new("time", Tense::Future, Body::of(serde_json::json!({"who":"me"}))));
        assert_eq!(town.present.borrow()["theme"].value()["bg"], "#111");
        assert!(!town.present.borrow().contains_key("key"));
        assert!(!town.present.borrow().contains_key("time"));
        assert_eq!(std::fs::read_to_string(&path).unwrap().lines().count(), 1);
        std::fs::remove_file(&path).ok();
    }
}
