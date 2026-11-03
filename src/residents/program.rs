//! A program: a file town runs, written in anything. Its first line says what it is, then words go
//! both ways, in the language it speaks: JSON, a line each, if it names none. It runs in its own
//! folder and its own process group, so all it starts ends with it.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::rc::Rc;

use tokio::process::{Child, ChildStdin, ChildStdout};
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::mpsc;

use super::{hears, named, watch_for, Tasks, Watched};
use crate::connection::{of, say, Heard, Speech};
use crate::language::Language;
use crate::parts::Parts;
use crate::town::Town;
use crate::unix;
use crate::word::Word;

/// What a runnable resident says first, to be one: a Lua resident's table, as a line of JSON.
/// `kinds` it hears; perhaps `watch` and `own`, as a Lua resident has them. Nothing else: a
/// misspelled field would hear nothing, and say nothing of why.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Declaration {
    #[serde(default)]
    kinds: Vec<String>,
    #[serde(default)]
    watch: std::collections::BTreeMap<String, Parts>,
    /// the language it speaks in, after its first line; JSON if it does not say
    #[serde(default)]
    speaks: Option<String>,
    #[serde(default)]
    own: Vec<String>,
}

/// A runnable resident, run once in its own folder, until it ends or the town opens again. One
/// that fails says how, and stays ended, as one that ends well does: to run it again is to save it
/// again.
pub(super) async fn residing(town: Rc<Town>, path: PathBuf) {
    let from = named(&path);
    // a group of its own, so all it starts ends with it (`Group`); kill_on_drop reaps it
    let child = tokio::process::Command::new(&path)
        .current_dir(path.parent().unwrap_or(Path::new("/")))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .kill_on_drop(true)
        .spawn();
    match child {
        Err(e) => town.error(&from, None, format!("cannot run: {e}")),
        Ok(child) => reside(&town, &from, &path, child).await,
    }
}

/// A run of a runnable resident: `live`, and each line on its stderr an `error`, every one heard,
/// though it said them as it ended.
async fn reside(town: &Rc<Town>, from: &str, path: &Path, mut child: Child) {
    let group = Group::of(town, &child);
    let (Some(out), Some(inp), Some(err)) =
        (child.stdout.take(), child.stdin.take(), child.stderr.take())
    else {
        return;
    };
    let mut stderr = Tasks(vec![tokio::task::spawn_local({
        let (town, from) = (town.clone(), from.to_string());
        async move {
            let mut heard = Heard::new(err);
            while let Some(line) = heard.next().await {
                town.error(&from, None, line);
            }
        }
    })]);
    live(town, from, path, &mut child, out, inp).await;
    // all it started ends with it, so its stderr closes, and the last of it is heard
    drop(group);
    for h in &mut stderr.0 {
        h.await.ok();
    }
}

/// A run, from its first line to its end. Its first line declares it; then it hears the present
/// of its kinds and each word of them since it started, and what it says is said in town. What it
/// watches and owns is its own while it runs.
async fn live(
    town: &Rc<Town>,
    from: &str,
    path: &Path,
    child: &mut Child,
    out: ChildStdout,
    mut inp: ChildStdin,
) {
    // heard from the moment it starts, not from when it has said what it is: the present as of
    // now, then every word after, so a fact is heard once
    let (mut words, greeting) = (town.words.subscribe(), town.present_words());
    let mut heard = Heard::new(out);
    let Some(first) = heard.next().await else {
        if let Err(how) = how(child.wait().await) {
            town.error(from, None, format!("ended before it said what it is: {how}"));
        }
        return;
    };
    let declared = serde_json::from_str::<Declaration>(&first).map_err(|e| e.to_string());
    let declaration = declared.and_then(|d| {
        let watch: Result<Vec<_>, String> =
            d.watch.iter().map(|(k, w)| Ok((k.clone(), Watched::of(w)?))).collect();
        Ok((d, watch?))
    });
    let (declaration, watch) = match declaration {
        Ok(d) => d,
        Err(e) => {
            return town.error(from, None, format!("its first line must say what it is: {e}"))
        }
    };
    // its language, once it is said: until then what it says is heard as far as it is words in
    // JSON, so it may say its language itself, and the first of what is not is held for its
    // language to read; and one that ends while it waits is heard to end
    let speaks = declaration.speaks.as_deref().unwrap_or("json");
    let waiting = town.language(speaks, from);
    tokio::pin!(waiting);
    let (json, mut in_json) = (Language::json(), true);
    let word = |p: &Parts| of(p).ok();
    let language = loop {
        tokio::select! {
            biased;
            language = &mut waiting => match language {
                Some(language) => break language,
                None => return,
            },
            said = heard.unit_if(&json, &town.languages, word), if in_json => match said {
                Some(Some(w)) => town.talk(w),
                Some(None) => in_json = false,
                None => {
                    if let Err(how) = how(child.wait().await) {
                        town.error(from, None, format!("ended: {how}"));
                    }
                    return;
                }
            },
            status = child.wait() => {
                if let Err(how) = how(status) {
                    town.error(from, None, format!("ended: {how}"));
                }
                return;
            }
        }
    };
    // written aside, so a program slow to read never stops it being heard; but only so much
    let (tx, mut rx) = mpsc::channel::<Word>(1024);
    let (writes, me, mine) = (town.clone(), from.to_string(), language.clone());
    let mut tasks = Tasks(vec![tokio::task::spawn_local(async move {
        while let Some(w) = rx.recv().await {
            match say(&mut inp, &w, &mine, &writes.languages).await {
                Ok(true) => {}
                Ok(false) => {
                    writes.error(&me, Some(&w.kind), "cannot be said in its language".into())
                }
                Err(_) => return,
            }
        }
    })]);
    let listens = |said: &str| declaration.kinds.iter().any(|kind| hears(kind, said));
    let mut hand = Hand { tx, told: false };
    for w in greeting.into_iter().filter(|w| listens(&w.kind)) {
        hand.over(town, from, w);
    }
    // the kinds it made its own, held while it runs, given back as it ends
    let own: Vec<String> =
        declaration.own.iter().chain(declaration.watch.keys()).cloned().collect();
    declaration.own.iter().for_each(|kind| town.hold(kind));
    for (kind, file) in watch {
        tasks.0.push(watch_for(town, from, path, kind, file));
    }
    let (mut speech, mut talking) = (Speech::new(heard, language), true);
    loop {
        tokio::select! {
            // what it said before it ended is heard before its end
            biased;
            said = speech.next(town, from), if talking => match said {
                Some(w) => town.talk(w),
                None => talking = false,
            },
            w = words.recv() => match w {
                Ok(w) => {
                    // never its own failings: one it hears and says again would be heard again
                    // and a body is read only for an error
                    let its_own = || w.body.part("from").and_then(Parts::text) == Some(from);
                    if listens(&w.kind) && !(w.kind == "error" && its_own()) {
                        hand.over(town, from, w);
                    }
                }
                Err(RecvError::Lagged(n)) => {
                    town.error(from, None, format!("missed {n} words: it fell behind"))
                }
                Err(RecvError::Closed) => break,
            },
            status = child.wait() => {
                if let Err(how) = how(status) {
                    town.error(from, None, format!("ended: {how}"));
                }
                break;
            }
        }
    }
    own.iter().for_each(|kind| town.free(kind));
}

/// Whether a run ended well, and if not, how it ended.
fn how(status: std::io::Result<std::process::ExitStatus>) -> Result<(), String> {
    match status {
        Ok(s) if s.success() => Ok(()),
        Ok(s) => Err(s.to_string()),
        Err(e) => Err(e.to_string()),
    }
}

/// The process group of a run: the program and all it starts, ended as one when the run ends,
/// however it ends. Each is noted in its town while it lives, so the town reaches them all.
pub(crate) struct Group(Rc<Town>, unix::Pid);

impl Group {
    pub(crate) fn of(town: &Rc<Town>, child: &Child) -> Option<Group> {
        let id = unix::Pid::try_from(child.id()?).ok()?;
        town.groups.borrow_mut().insert(id);
        Some(Group(town.clone(), id))
    }
}

impl Drop for Group {
    fn drop(&mut self) {
        // one the town has ended already is not ended twice
        if self.0.groups.borrow_mut().remove(&self.1) {
            unix::end_group(self.1);
        }
    }
}

/// End the group of every program running in a town.
pub(super) fn end_groups(town: &Town) {
    town.groups.borrow_mut().drain().for_each(unix::end_group);
}

/// Wait for the town to be stopped, end every program's group, since a signal to the town's own
/// group does not reach theirs, and end as a shell says that signal ended it: 128 and its number.
pub async fn ending(town: Rc<Town>) {
    use tokio::signal::unix::{signal, SignalKind};
    // a signal the town was started to ignore (`nohup`, a job in the background) it still ignores:
    // to listen for it would undo that
    let on = |kind: SignalKind| {
        let ignored = unix::ignored(kind.as_raw_value());
        (!ignored).then(|| signal(kind).expect("signals are heard"))
    };
    let heard = |s: Option<tokio::signal::unix::Signal>| async move {
        match s {
            Some(mut s) => _ = s.recv().await,
            None => std::future::pending().await,
        }
    };
    let (term, int, hup) = (SignalKind::terminate(), SignalKind::interrupt(), SignalKind::hangup());
    let sig = tokio::select! {
        _ = heard(on(term)) => term,
        _ = heard(on(int)) => int,
        _ = heard(on(hup)) => hup,
    };
    end_groups(&town);
    std::process::exit(128 + sig.as_raw_value()) // as a shell says a signal ended it
}

/// The words a run is handed, on its stdin, in its language. One that does not read them has, at
/// most, so many waiting; those after are dropped, and that is said once a run.
struct Hand {
    tx: mpsc::Sender<Word>,
    told: bool,
}

impl Hand {
    fn over(&mut self, town: &Town, from: &str, w: Word) {
        // closed, not full: it stopped reading for good, and how it ended is said
        if let Err(mpsc::error::TrySendError::Full(_)) = self.tx.try_send(w) {
            if !std::mem::replace(&mut self.told, true) {
                town.error(from, None, "is not reading: words dropped".into());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::body::Body;
    use crate::testing::{guard, until, upto};
    use crate::word::Tense;

    use crate::residents::inside::{end, script};
    use crate::residents::open;
    use mlua::Lua;
    use std::time::Duration;

    // a program that waits for its language, and ends while it waits, is heard to end
    #[test]
    fn a_program_waiting_for_its_language_may_end() {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        tokio::task::LocalSet::new().block_on(&rt, async {
            let dir = std::env::temp_dir().join(format!("town-unsaid-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            let typo = script(&dir, "typo", r#"echo '{"speaks":"wrds"}'; exit 3"#);
            let town = Town::with_log(dir.join("past.log"));
            let mut words = town.words.subscribe();
            open(&Lua::new(), &town, &[typo]);
            let from = |w: &Word| w.kind == "error" && w.body.value()["from"] == "typo";
            let waits = until(&mut words, from).await.body.value()["message"].clone();
            assert_eq!(waits, "no language is called wrds yet: it waits for one");
            let ended = until(&mut words, from).await.body.value()["message"].clone();
            assert_eq!(ended, "ended: exit status: 3");
            // one that says its own language, in JSON as it waits, then speaks it
            let grammar = r#"{"seq":[{"part":["kind",{"upto":"\n"}]}]}"#;
            let language = format!(r#"{{"name":"bare","grammar":{grammar}}}"#);
            let language = format!(r#"{{"kind":"language","tense":"past","body":{language}}}"#);
            let first = r#"echo '{"speaks":"bare"}'"#;
            let says = format!("{first}\nprintf '%s\\n' '{language}'\necho hello; exec sleep 60");
            open(&Lua::new(), &town, &[script(&dir, "bare", &says)]);
            until(&mut words, |w| w.kind == "hello").await;
            // one that speaks a language said after it: what it says first, JSON but no word, is
            // held for its language, and heard in it once the language is said
            let early = r#"echo '{"speaks":"late"}'; echo 123; echo second; exec sleep 60"#;
            open(&Lua::new(), &town, &[script(&dir, "early", early)]);
            let from = |w: &Word| w.kind == "error" && w.body.value()["from"] == "early";
            let waits = until(&mut words, from).await.body.value()["message"].clone();
            assert_eq!(waits, "no language is called late yet: it waits for one");
            let grammar: serde_json::Value = serde_json::from_str(grammar).unwrap();
            let late = serde_json::json!({ "name": "late", "grammar": grammar });
            town.talk(Word::new("language", Tense::Past, Body::of(late)));
            let heard = upto(&mut words, |w| w.kind == "second");
            let heard: Vec<_> = heard.await.into_iter().map(|w| w.kind).collect();
            assert_eq!(heard, ["language", "123", "second"], "JSON, but no word: {heard:?}");
            end(&town);
            std::fs::remove_dir_all(&dir).ok();
        });
    }

    // a program runs once: one that fails says why, on stderr and in how it ended; one that ends
    // well says nothing; one that never says what it is, or ends before it does, says so. None runs
    // again. What it owns is its own only while it runs
    #[test]
    fn a_program_runs_once() {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        tokio::task::LocalSet::new().block_on(&rt, async {
            let dir = std::env::temp_dir().join(format!("town-ran-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            let began = r#"echo '{}'; echo '{"kind":"began","tense":"past"}'"#;
            let failing = script(&dir, "failing", &format!("{began}; echo oops >&2; exit 3"));
            let ending = r#"echo '{"kinds":["go"],"own":["mine"]}'
                echo '{"kind":"up","tense":"past"}'; read -r _"#;
            let ending = script(&dir, "ending", ending);
            let mute = script(&dir, "mute", r#"echo '{"kind":"began"}'"#);
            let early = script(&dir, "early", "exit 4");
            let town = Town::with_log(dir.join("past.log"));
            let mut words = town.words.subscribe();
            open(&Lua::new(), &town, &[failing, ending, mute, early]);
            let mut runs: Vec<_> = town.tasks.borrow_mut().drain(..).collect();
            // ending owns what it says it does while it runs, and gives it back as it ends
            let mine = Word::new("mine", Tense::Past, Body::none());
            let mut said = upto(&mut words, |w| w.kind == "up").await;
            assert!(!town.admits(&mine), "its own, while it runs");
            town.talk(Word::new("go", Tense::Past, Body::none()));
            // every run ends, and its task with it: all they will ever say has been said
            for run in &mut runs {
                guard(run).await.unwrap();
            }
            assert!(town.admits(&mine), "given back");
            said.extend(std::iter::from_fn(|| words.try_recv().ok()));
            let from = |from: &str, is: &str| {
                said.iter()
                    .filter(|w| w.kind == "error" && w.body.value()["from"] == from)
                    .filter(|w| w.body.value()["message"].as_str().unwrap().starts_with(is))
                    .count()
            };
            assert_eq!(said.iter().filter(|w| w.kind == "began").count(), 1);
            assert_eq!(from("failing", ""), 2, "its stderr, and how it ended");
            assert_eq!(from("failing", "oops"), 1);
            assert_eq!(from("failing", "ended: exit status: 3"), 1);
            assert_eq!(from("ending", ""), 0);
            assert_eq!(from("mute", ""), 1);
            assert_eq!(from("mute", "its first line must say what it is"), 1);
            assert_eq!(from("early", ""), 1);
            assert_eq!(from("early", "ended before it said what it is: exit status: 4"), 1);
            runs.iter().for_each(|r| r.abort());
            std::fs::remove_dir_all(&dir).ok();
        });
    }

    // a program hears the present of its kinds, then each word of them, and nothing else; it
    // watches and owns as a Lua resident does, its paths beside it
    #[test]
    fn a_program_hears_its_kinds() {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        tokio::task::LocalSet::new().block_on(&rt, async {
            let dir = std::env::temp_dir().join(format!("town-hears-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("colors"), "bg=#000").unwrap();
            let parrot = script(
                &dir,
                "parrot",
                r#"echo '{"kinds":["colors","ping"],"watch":{"colors":"colors"},"own":["heard"]}'
                while read -r w; do
                  printf '{"kind":"heard","tense":"past","body":%s}\n' "$w"
                done"#,
            );
            let town = Town::with_log(dir.join("past.log"));
            town.talk(Word::new("theme", Tense::Present, Body::of("dark")));
            let mut words = town.words.subscribe();
            open(&Lua::new(), &town, &[parrot]);
            let heard = |w: &Word| w.kind == "heard";
            assert_eq!(
                until(&mut words, heard).await.body.value()["kind"],
                "colors",
                "its watch, not the theme"
            );
            // a pong, then a ping: it hears in order, so a pong heard would come first
            town.talk(Word::new("pong", Tense::Past, Body::none()));
            town.talk(Word::new("ping", Tense::Past, Body::none()));
            assert_eq!(until(&mut words, heard).await.body.value()["kind"], "ping");
            assert!(!town.admits(&Word::new("heard", Tense::Past, Body::none())));
            assert!(!town.admits(&Word::new("colors", Tense::Present, Body::none())));
            end(&town);
            std::fs::remove_dir_all(&dir).ok();
        });
    }

    // a program never hears its own failings, so one that says again what it hears ends; and one
    // whose first line is no text is told it is no resident
    #[test]
    fn a_program_does_not_hear_itself_fail() {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        tokio::task::LocalSet::new().block_on(&rt, async {
            let dir = std::env::temp_dir().join(format!("town-itself-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            let loud = script(
                &dir,
                "loud",
                r#"echo '{"kinds":["*","error"]}'; while read -r w; do echo "$w" >&2; done"#,
            );
            let binary = script(&dir, "binary", r"printf '\377\n'; exec sleep 60");
            let town = Town::with_log(dir.join("past.log"));
            let mut words = town.words.subscribe();
            open(&Lua::new(), &town, &[loud, binary]);
            let failed = |from: &'static str| {
                move |w: &Word| w.kind == "error" && w.body.value()["from"] == from
            };
            let told = until(&mut words, failed("binary")).await;
            assert!(told.body.value()["message"].as_str().unwrap().starts_with("its first line"));
            // loud hears the binary's failing, but not its own saying of it: the next it says is
            // pong
            until(&mut words, failed("loud")).await;
            town.talk(Word::new("pong", Tense::Past, Body::none()));
            let next = until(&mut words, failed("loud")).await;
            assert!(next.body.value()["message"].as_str().unwrap().contains("pong"), "{next:?}");
            end(&town);
            std::fs::remove_dir_all(&dir).ok();
        });
    }

    // a program that does not read has only so many words waiting: what is dropped is said, once
    #[test]
    fn a_program_that_does_not_read_is_said_once() {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        tokio::task::LocalSet::new().block_on(&rt, async {
            let dir = std::env::temp_dir().join(format!("town-deaf-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            let deaf =
                r#"echo '{"kinds":["ping"]}'; echo '{"kind":"up","tense":"past"}'; exec sleep 60"#;
            let deaf = script(&dir, "deaf", deaf);
            let town = Town::with_log(dir.join("past.log"));
            let mut words = town.words.subscribe();
            open(&Lua::new(), &town, &[deaf]);
            until(&mut words, |w| w.kind == "up").await;
            let dropped = |w: &Word| w.body.value()["message"] == "is not reading: words dropped";
            let (mut said, mut pings) = (0, 0);
            // pings until the pipe and the queue are full and it is said; then as many again
            while said == 0 || pings < 20_000 {
                town.talk(Word::new("ping", Tense::Past, Body::none()));
                pings += 1;
                // every word heard by all, so it falls behind no one
                while !town.words.is_empty() {
                    tokio::task::yield_now().await;
                    said += std::iter::from_fn(|| words.try_recv().ok()).filter(dropped).count();
                }
                assert!(pings < 100_000, "never said");
            }
            assert_eq!(said, 1);
            end(&town);
            std::fs::remove_dir_all(&dir).ok();
        });
    }

    // all a program starts ends with it: a child it left running, as a reopen ends the run
    #[test]
    fn a_programs_processes_end_with_it() {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        tokio::task::LocalSet::new().block_on(&rt, async {
            let dir = std::env::temp_dir().join(format!("town-group-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            let parent = r#"sleep 60 & echo $! > child
                echo '{}'; echo '{"kind":"up","tense":"past"}'; exec cat"#;
            let parent = script(&dir, "parent", parent);
            let town = Town::with_log(dir.join("past.log"));
            let mut words = town.words.subscribe();
            open(&Lua::new(), &town, &[parent]);
            until(&mut words, |w| w.kind == "up").await;
            let child: unix::Pid =
                std::fs::read_to_string(dir.join("child")).unwrap().trim().parse().unwrap();
            let alive = || unix::alive(child);
            assert!(alive(), "running while the program runs");
            end(&town);
            guard(async {
                while alive() {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await;
            std::fs::remove_dir_all(&dir).ok();
        });
    }
}
