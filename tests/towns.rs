//! Towns joining towns: real towns, each its own process and square, one joined into another by
//! `town '{"name":"<name>"}'`, as a town on another machine is over ssh.
//!   cargo test --test towns

use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;

use serde_json::Value;

use town::testing::{scratch, script, Scratch, GUARD};

const TOWN: &str = env!("CARGO_BIN_EXE_town");

/// A town running: its home, where its square and log are, and its residents' folder.
struct Running {
    home: PathBuf,
    folder: PathBuf,
    town: Option<Child>,
}

impl Running {
    /// a town named `name` in `dir`, its residents these scripts, by name; not yet begun
    fn new(dir: &Scratch, name: &str, residents: &[(&str, String)]) -> Running {
        let (home, folder) = (dir.join(format!("{name}/home")), dir.join(format!("{name}/town")));
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&folder).unwrap();
        for (file, body) in residents {
            script(&folder, file, body);
        }
        Running { home, folder, town: None }
    }

    /// begun, once its square is there
    fn begin(mut self) -> Running {
        self.start();
        self
    }

    fn start(&mut self) {
        let mut town = Command::new(TOWN);
        town.arg(&self.folder).env("TOWN_HOME", &self.home).stderr(Stdio::null());
        self.town = Some(town.spawn().unwrap());
        // `town` waits for a square: told `now`, the town is up
        drop(self.listen(&["town/residents"]).ready());
    }

    /// stopped, as a machine goes down: its programs see their input end
    fn stop(&mut self) {
        if let Some(mut town) = self.town.take() {
            let _ = town.kill();
            let _ = town.wait();
        }
    }

    /// `town` here, at its square, saying `first` first
    fn town(&self, first: &Value) -> Command {
        let mut town = Command::new(TOWN);
        town.arg(first.to_string()).env("TOWN_HOME", &self.home).stdin(Stdio::piped());
        town
    }

    /// say a word here: its tense, if one is named, `present` if not, its kind, then each
    /// `key=value` of its body; a fact with none is a fact of nothing
    fn talk(&self, word: &[&str]) {
        use std::io::Write;
        let tenses = ["past", "present", "future"];
        let (tense, rest) = match word.first() {
            Some(t) if tenses.contains(t) => (*t, &word[1..]),
            _ => ("present", word),
        };
        let body: serde_json::Map<String, Value> = rest[1..]
            .iter()
            .filter_map(|p| p.split_once('='))
            .map(|(k, v)| (k.to_string(), Value::from(v)))
            .collect();
        let body = if body.is_empty() { Value::Null } else { Value::Object(body) };
        let said = serde_json::json!({ "kind": rest[0], "tense": tense, "body": body });
        let mut talk = self.town(&serde_json::json!({})).stdout(Stdio::null()).spawn().unwrap();
        let mut to = talk.stdin.take().unwrap();
        writeln!(to, "{said}").unwrap();
        drop(to);
        assert!(talk.wait().unwrap().success(), "said: {word:?}");
    }

    /// what one listening for `kinds` here hears, each word as it comes, while this is held
    fn listen(&self, kinds: &[&str]) -> Heard {
        self.heard(&serde_json::json!({ "kinds": kinds }))
    }

    /// what `town` here, saying `first` first, is told, each word as it comes
    fn heard(&self, first: &Value) -> Heard {
        let mut child = self.town(first).stdout(Stdio::piped()).spawn().unwrap();
        let out = child.stdout.take().unwrap();
        let (to, words) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(out).lines().map_while(Result::ok) {
                let Ok(w) = serde_json::from_str::<Value>(&line) else { continue };
                if to.send(w).is_err() {
                    break;
                }
            }
        });
        Heard { child, words, before: Default::default() }
    }

    /// the kinds of the facts here now
    fn present(&self) -> Vec<String> {
        let mut heard = self.listen(&["/"]);
        let mut kinds = Vec::new();
        loop {
            let w = heard.next().expect("the present, then now");
            if w["kind"] == "" {
                return kinds;
            }
            kinds.push(w["kind"].as_str().unwrap().to_string());
        }
    }

    /// the program that joins `there` at `kind`, coming to its square by `name`, as `ssh` would,
    /// and joins it again as that join ends
    fn joins(there: &Running, kind: &str, name: &str) -> String {
        let square = serde_json::json!({
            "kind": "town/join", "tense": "future",
            "body": { "as": kind, "argv": [
                "env", format!("TOWN_HOME={}", there.home.display()), TOWN,
                serde_json::json!({ "name": name }).to_string(),
            ]},
        });
        format!(
            r#"echo '{{"kinds":["town/join"]}}'; echo '{square}'
            while read -r w; do
                case "$w" in
                    *'"code"'*'"as":"{kind}"'*|*'"as":"{kind}"'*'"code"'*) echo '{square}';;
                esac
            done"#
        )
    }

    /// the program that gives `name` to another town, hearing `kinds`
    fn names(name: &str, kinds: &[&str]) -> String {
        Running::names_but(name, kinds, &[])
    }

    /// the program that gives `name` to another town, hearing `kinds` but those it `except`s
    fn names_but(name: &str, kinds: &[&str], except: &[&str]) -> String {
        let names = serde_json::json!({ "names": { name: { "kinds": kinds, "except": except } } });
        format!("echo '{names}'; exec cat >/dev/null")
    }
}

impl Drop for Running {
    /// ended as one ends a town: it ends its programs' groups, and goes
    fn drop(&mut self) {
        if let Some(mut town) = self.town.take() {
            let pid = libc::pid_t::try_from(town.id()).unwrap();
            // SAFETY: a signal to a child of ours; it touches no memory of ours
            unsafe { libc::kill(pid, libc::SIGTERM) };
            let _ = town.wait();
        }
    }
}

/// what one listening hears, and what it heard before it was ready
struct Heard {
    child: Child,
    words: mpsc::Receiver<Value>,
    before: std::collections::VecDeque<Value>,
}

impl Heard {
    /// the next word, or none once its `town` ends, or once `GUARD` passes, so a test that goes
    /// wrong fails and does not hang
    fn next(&mut self) -> Option<Value> {
        self.before.pop_front().or_else(|| self.words.recv_timeout(GUARD).ok())
    }

    /// once it is told `now`: from then, it hears each word said, so what is said after is heard.
    /// What it heard before is kept.
    fn ready(mut self) -> Heard {
        loop {
            let w = self.words.recv_timeout(GUARD).expect("now, before it ended");
            if w["kind"] == "" {
                return self;
            }
            self.before.push_back(w);
        }
    }

    /// once the facts it has heard, folded as they come, `are` so: each fact it has, by kind
    fn has(&mut self, are: impl Fn(&std::collections::BTreeSet<String>) -> bool) {
        let mut facts = std::collections::BTreeSet::new();
        while !are(&facts) {
            let w = self.next().unwrap_or_else(|| panic!("heard, before it ended: {facts:?}"));
            if w["tense"] == "present" {
                let kind = w["kind"].as_str().unwrap().to_string();
                match w["body"].is_null() {
                    true => facts.remove(&kind),
                    false => facts.insert(kind),
                };
            }
        }
    }

    /// how its `town` exits once it ends by itself; one that never ends fails after `GUARD`
    fn exits(mut self) -> std::process::ExitStatus {
        loop {
            match self.words.recv_timeout(GUARD) {
                Ok(_) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => return self.child.wait().unwrap(),
                Err(mpsc::RecvTimeoutError::Timeout) => panic!("its town never ended"),
            }
        }
    }

    /// the first word `is`, before its `town` ends
    fn until(&mut self, is: impl Fn(&Value) -> bool) -> Value {
        loop {
            let w = self.next().expect("heard, before it ended");
            if is(&w) {
                return w;
            }
        }
    }
}

impl Drop for Heard {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// a word's kind is `kind`
fn kind(kind: &'static str) -> impl Fn(&Value) -> bool {
    move |w: &Value| w["kind"] == kind
}

// a town joined into another is a join there: what it says is heard under its kind, and what is
// wanted there is wanted of it; it comes by a name it gives, and by no other
#[test]
fn a_town_joins_another_by_name() {
    let dir = scratch();
    let other =
        Running::new(&dir, "leeds.example", &[("names", Running::names("luke.example", &["/"]))])
            .begin();
    // one of luke's own that wants of the other town, as it is told to
    let asks = r#"echo '{"kinds":["go"]}'
        while read -r w; do
            case "$w" in
                *'"kind":"go"'*) echo '{"kind":"leeds.example/x","tense":"future","body":1}';;
            esac
        done"#;
    let joins = Running::joins(&other, "leeds.example", "luke.example");
    // one that joins the other town by a name none gives, once it is told to
    let nobody = serde_json::json!({
        "kind": "town/join", "tense": "future",
        "body": { "as": "nobody.example", "argv": [
            "env", format!("TOWN_HOME={}", other.home.display()), TOWN,
            serde_json::json!({ "name": "nobody.example" }).to_string(),
        ]},
    });
    let nobody = format!(
        r#"echo '{{"kinds":["come"]}}'
        while read -r w; do case "$w" in *'"kind":"come"'*) echo '{nobody}';; esac; done"#
    );
    let residents = [("asks", asks.to_string()), ("joins", joins), ("nobody.example", nobody)];
    let luke = Running::new(&dir, "luke.example", &residents).begin();
    let mut theme = luke.listen(&["leeds.example/theme"]);
    other.talk(&["theme", "bg=dark"]);
    let heard = theme.until(kind("leeds.example/theme"));
    assert_eq!(heard["body"]["bg"], "dark", "its own, under its kind");

    let mut wanted = other.listen(&["x"]).ready();
    luke.talk(&["past", "go"]);
    let want = wanted.until(kind("x"));
    assert_eq!((want["tense"].as_str(), &want["body"]), (Some("future"), &Value::from(1)));

    // the other town's errors are its words, heard under the join's kind
    let mut errors = luke.listen(&["nobody.example/town/error"]).ready();
    luke.talk(&["past", "come"]);
    let refused = errors.until(|w| {
        w["kind"] == "nobody.example/town/error"
            && w["body"]["message"].as_str().unwrap().contains("nobody.example")
    });
    assert_eq!(
        refused["body"]["message"], "no resident names `nobody.example` yet: it waits for one",
        "a name none gives"
    );
}

/// `at` joins `there` at `kind`, coming to its square by `name`, as its own resident `joins`
fn joining(at: &Running, there: &Running, kind: &str, name: &str) {
    script(&at.folder, "joins", &Running::joins(there, kind, name));
}

// a name gives what it hears, but what it excepts: one that comes by it is told `open`, and never
// `secret`. `secret`, said first, would be told first
#[test]
fn a_name_gives_its_kinds_but_what_it_excepts() {
    let dir = scratch();
    let (a, b) = ("a.example", "b.example");
    let gives = Running::names_but(b, &["/"], &["secret"]);
    let ta = Running::new(&dir, a, &[("names", gives)]);
    let tb = Running::new(&dir, b, &[]);
    joining(&tb, &ta, a, b);
    let (ta, tb) = (ta.begin(), tb.begin());
    let mut in_b = tb.listen(&[&format!("/{a}")]).ready();
    ta.talk(&["secret", "v=1"]);
    ta.talk(&["open", "v=1"]);
    in_b.has(|f| f.contains(&format!("{a}/open")));
    assert!(!tb.present().contains(&format!("{a}/secret")), "not what it excepts");
}

// two towns that each hear all of the other are a pair: each joins the other at the name the other
// gives it, and none is told what passes through where it stands, so each word crosses once and
// none comes back. A word said after, heard across, shows the way is drained
#[test]
fn two_towns_that_hear_all_of_each_other_hear_each_word_once() {
    let dir = scratch();
    let a = Running::new(&dir, "a.example", &[("names", Running::names("b.example", &["/"]))]);
    let b = Running::new(&dir, "b.example", &[("names", Running::names("a.example", &["/"]))]);
    joining(&a, &b, "b.example", "a.example");
    joining(&b, &a, "a.example", "b.example");
    let (a, b) = (a.begin(), b.begin());
    let (mut in_a, mut in_b) =
        (a.listen(&["/b.example"]).ready(), b.listen(&["/a.example"]).ready());
    a.talk(&["x", "v=1"]);
    in_b.has(|f| f.contains("a.example/x"));
    b.talk(&["after", "v=1"]);
    a.talk(&["after", "v=1"]);
    in_a.has(|f| f.contains("b.example/after"));
    in_b.has(|f| f.contains("a.example/after"));
    let (in_a, in_b) = (a.present(), b.present());
    assert!(!in_a.iter().any(|k| passes_again(k, "a.example")), "none back to a: {in_a:?}");
    assert!(!in_b.iter().any(|k| passes_again(k, "b.example")), "none back to b: {in_b:?}");
}

/// whether a kind's path, the domains it begins with, names the domain `d`, or any domain twice:
/// what none may be told
fn passes_again(kind: &str, d: &str) -> bool {
    let parts: Vec<&str> = kind.split('/').take_while(|p| p.contains('.')).collect();
    parts.contains(&d) || (1..parts.len()).any(|i| parts[..i].contains(&parts[i]))
}

/// towns in a ring, each joined at its domain by the one before, which comes by its own domain and
/// hears all: the first says `x`, which goes round once, as far as the town after it. A word said
/// after, heard across by the first, shows the way back to it is drained. Then none has a path
/// through its own domain, nor one through a domain twice
fn a_ring_stops(domains: &[&str]) {
    let dir = scratch();
    let n = domains.len();
    let towns: Vec<Running> = (0..n)
        .map(|i| {
            let before = domains[(i + n - 1) % n];
            Running::new(&dir, domains[i], &[("names", Running::names(before, &["/"]))])
        })
        .collect();
    for i in 0..n {
        let next = (i + 1) % n;
        joining(&towns[i], &towns[next], domains[next], domains[i]);
    }
    let towns: Vec<Running> = towns.into_iter().map(Running::begin).collect();
    // the copy that has gone furthest: at the town after the first, through every other
    let furthest = (2..n).map(|i| domains[i]).chain([domains[0]]).collect::<Vec<_>>();
    let furthest = format!("{}/x", furthest.join("/"));
    let mut far = towns[1].listen(&[&format!("/{}", domains[2])]).ready();
    let mut back = towns[0].listen(&[&format!("/{}", domains[1])]).ready();
    towns[0].talk(&["x", "v=1"]);
    far.has(|f| f.contains(&furthest));
    towns[1].talk(&["after", "v=1"]);
    back.has(|f| f.contains(&format!("{}/after", domains[1])));
    for (town, d) in towns.iter().zip(domains) {
        let present = town.present();
        let again: Vec<&String> = present.iter().filter(|k| passes_again(k, d)).collect();
        assert!(again.is_empty(), "{d} holds a path through itself or a domain twice: {again:?}");
    }
}

// three towns in a ring, each hearing all of the next, stop: what one says goes round once, and
// none is told a path through where it stands
#[test]
fn three_towns_in_a_ring_stop() {
    a_ring_stops(&["a.example", "b.example", "c.example"]);
}

#[test]
fn four_towns_in_a_ring_stop() {
    a_ring_stops(&["a.example", "b.example", "c.example", "d.example"]);
}

// what a town says again is its copy, under its domain, and never goes back to where it began:
// york hears leeds, luke hears york, and leeds hears luke, but is not told york's copy of its own
// theme. A word luke says after, heard by leeds, shows the way is drained
#[test]
fn a_copy_never_goes_back() {
    let dir = scratch();
    let (leeds, york, luke) = ("leeds.example", "york.example", "luke.example");
    let gives = |name: &str| Running::names(name, &["/"]);
    let l = Running::new(&dir, leeds, &[("names", gives(york))]);
    let y = Running::new(&dir, york, &[("names", gives(luke))]);
    let k = Running::new(&dir, luke, &[("names", gives(leeds))]);
    joining(&y, &l, leeds, york);
    joining(&k, &y, york, luke);
    joining(&l, &k, luke, leeds);
    let (l, _y, k) = (l.begin(), y.begin(), k.begin());
    let mut in_luke = k.listen(&[&format!("/{york}")]).ready();
    let mut in_leeds = l.listen(&[&format!("/{luke}")]).ready();
    l.talk(&["theme", "bg=dark"]);
    in_luke.has(|f| f.contains(&format!("{york}/{leeds}/theme")));
    k.talk(&["after", "v=1"]);
    in_leeds.has(|f| f.contains(&format!("{luke}/after")));
    let back = format!("{luke}/{york}/{leeds}/theme");
    assert!(!l.present().contains(&back), "leeds is not told its own, back");
}

// a path passes through a domain only where the domain is one of its parts, whole: a kind that
// names it in part, `leeds.example.old/x` or `x/leeds`, is told to the one that stands there
#[test]
fn a_part_of_a_domain_is_not_the_domain() {
    let dir = scratch();
    let (leeds, luke) = ("leeds.example", "luke.example");
    let l = Running::new(&dir, leeds, &[]);
    let k = Running::new(&dir, luke, &[("names", Running::names(leeds, &["/"]))]);
    joining(&l, &k, luke, leeds);
    let (l, k) = (l.begin(), k.begin());
    let mut in_leeds = l.listen(&[&format!("/{luke}")]).ready();
    k.talk(&["leeds.example.old/x", "v=1"]);
    k.talk(&["x/leeds", "v=1"]);
    in_leeds.has(|f| {
        f.contains(&format!("{luke}/leeds.example.old/x")) && f.contains(&format!("{luke}/x/leeds"))
    });
}

// what a town that joins another says of it is what that one says now: come again after a time
// away, what that one no longer has is nothing
#[test]
fn a_town_joined_again_holds_only_what_is_so() {
    let dir = scratch();
    let other =
        Running::new(&dir, "leeds.example", &[("names", Running::names("luke.example", &["/"]))])
            .begin();
    let mut luke = Running::new(
        &dir,
        "luke.example",
        &[("joins", Running::joins(&other, "leeds.example", "luke.example"))],
    );
    luke.start();
    let mut old = luke.listen(&["leeds.example/old"]);
    other.talk(&["old", "v=1"]);
    old.until(kind("leeds.example/old"));
    drop(old);
    luke.stop();
    // while luke is away, leeds lets `old` go, and has `fresh`
    other.talk(&["present", "old"]);
    other.talk(&["fresh", "v=1"]);
    luke.start();
    // what it held as it went, then, as leeds comes again, only what is so
    let mut heard = luke.listen(&["/leeds.example"]);
    heard.has(|f| f.contains("leeds.example/fresh") && !f.contains("leeds.example/old"));
}

// what a town's own program joined there says is the town's own, heard through `/`
#[test]
fn a_towns_own_join_is_its_own() {
    let dir = scratch();
    let local = serde_json::json!({
        "kind": "town/join", "tense": "future",
        "body": { "as": "local", "argv": [
            "sh", "-c", "echo \"$0\"; exec cat", r#"{"kind":"x","body":1}"#,
        ]},
    });
    let joins = format!("echo '{{}}'; echo '{local}'; exec cat >/dev/null");
    let names = Running::names("luke.example", &["/"]);
    let other = Running::new(&dir, "leeds.example", &[("names", names), ("joins", joins)]).begin();
    let luke = Running::new(
        &dir,
        "luke.example",
        &[("joins", Running::joins(&other, "leeds.example", "luke.example"))],
    );
    let mut x = luke.listen(&["leeds.example/local/x"]);
    let luke = luke.begin();
    assert_eq!(x.until(kind("leeds.example/local/x"))["body"], 1, "its own join's, through `/`");
    drop(luke);
}

// one that comes by a name hears what the name says now: as it changes, it comes again by it, and
// what it no longer hears is nothing
#[test]
fn a_town_hears_its_name_as_it_is_now() {
    let dir = scratch();
    let other =
        Running::new(&dir, "leeds.example", &[("names", Running::names("luke.example", &["a"]))]);
    let names = other.folder.join("names");
    // one that asks the town to open its names again, as a save does
    let open = serde_json::json!({
        "kind": "town/open", "tense": "future", "body": { "path": names },
    });
    let opens = format!(
        r#"echo '{{"kinds":["rename"]}}'
        while read -r w; do
            case "$w" in *'"kind":"rename"'*) echo '{open}';; esac
        done"#
    );
    script(&other.folder, "opens", &opens);
    let other = other.begin();
    let luke = Running::new(
        &dir,
        "luke.example",
        &[("joins", Running::joins(&other, "leeds.example", "luke.example"))],
    );
    let mut a = luke.listen(&["leeds.example/a"]);
    let luke = luke.begin();
    other.talk(&["a", "v=1"]);
    other.talk(&["b", "v=1"]);
    a.until(kind("leeds.example/a"));
    script(&other.folder, "names", &Running::names("luke.example", &["b"]));
    let mut heard = luke.listen(&["/leeds.example"]).ready();
    other.talk(&["past", "rename"]);
    heard.has(|f| f.contains("leeds.example/b") && !f.contains("leeds.example/a"));
}

// what one says to a town not yet come is kept: its input ended, it says it once the town comes,
// then goes, as it did all it was given
#[test]
fn a_word_said_before_a_town_comes_is_heard_as_it_comes() {
    use std::io::Write;
    let dir = scratch();
    let mut leeds = Running::new(&dir, "leeds.example", &[]);
    let mut talk = leeds.heard(&serde_json::json!({}));
    let mut to = talk.child.stdin.take().unwrap();
    writeln!(to, r#"{{"kind":"x","tense":"present","body":1}}"#).unwrap();
    drop(to);
    leeds.start();
    leeds.listen(&["x"]).has(|f| f.contains("x"));
    assert!(talk.exits().success(), "it did all it was given");
}
