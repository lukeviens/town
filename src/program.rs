//! A program: a file town runs, written in anything, a resident at the root. The README's *as a
//! program*.
//!
//! - **what it is**: its first line says so; then words go both ways, in the language it speaks.
//! - **where**: its own folder and process group, so all it starts ends with it.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::rc::Rc;
use std::time::Duration;

use tokio::process::{Child, ChildStdin, ChildStdout};

use crate::town::{Resident, Town};
use crate::unix;
use crate::voice::{self, Unsaid, Voice};

/// A runnable resident, `who`, run once, until it ends, goes, or the town opens again. One that
/// fails says how; either way it stays ended until its file is saved, or the town opens again.
pub(crate) async fn residing(town: Rc<Town>, who: Resident, path: PathBuf) {
    let folder = path.parent().unwrap_or(Path::new("/"));
    let run = async |child: &mut Child, out, inp| run(&town, &who, child, out, inp).await;
    if let Err(e) = Group::run(&town, &who, &[&path], Some(folder), run).await {
        town.error(&who, None, format!("cannot run: {e}"));
    }
    // ended, it comes no more, nor those it spoke for: what they owned is held no longer
    let at = format!("{}/{}", crate::town::RESIDENTS, who.place());
    town.hold_only(|kind| {
        kind.strip_prefix(&at).is_none_or(|r| !r.is_empty() && !r.starts_with('/'))
    });
}

/// A run, from its first line, `voice::comes`, to its end, and how it ended. What it owns is its
/// own while it runs.
async fn run(
    town: &Rc<Town>,
    who: &Resident,
    child: &mut Child,
    out: ChildStdout,
    mut inp: ChildStdin,
) {
    // one that ends as it says what it is, or waits for its language, has ended, though what it
    // started may hold its output open
    let waits = async |message: &str| town.error(who, None, message.into());
    let came = tokio::select! {
        biased;
        came = voice::comes(town, who, Voice::new(out), waits) => came,
        _ = child.wait() => Err(Unsaid::Gone),
    };
    match came {
        Ok(resident) => resident.live(town, &mut inp, Some(child.wait())).await,
        Err(Unsaid::Refused(e)) => return town.error(who, None, e),
        Err(Unsaid::Mute) => {
            if let Err(how) = how(child.wait().await) {
                town.error(who, None, format!("ended before it said what it is: {how}"));
            }
            return;
        }
        // it ends as it waits for its language
        Err(Unsaid::Gone) => {}
    }
    if let Err(how) = how(child.wait().await) {
        town.error(who, None, format!("ended: {how}"));
    }
}

/// Whether a run ended well, and if not, how it ended.
fn how(status: std::io::Result<std::process::ExitStatus>) -> Result<(), String> {
    match status {
        Ok(s) if s.success() => Ok(()),
        Ok(s) => Err(s.to_string()),
        Err(e) => Err(e.to_string()),
    }
}

/// A run's process group: the program and all it starts, ended as one when the run ends, however
/// it ends. Each is noted in its town while it lives, so the town can end them all.
pub(crate) struct Group(Rc<Town>, unix::Pid);

impl Group {
    fn of(town: &Rc<Town>, child: &Child) -> Option<Group> {
        let id = unix::Pid::try_from(child.id()?).ok()?;
        town.groups.borrow_mut().insert(id);
        Some(Group(town.clone(), id))
    }

    /// Run `argv` in `folder`, or town's own, in a group of its own, and `run` with its stdout and
    /// stdin: all it starts ends as `run` ends or is dropped. Each line on its stderr is a
    /// `town/error` from `who`, its last ones too. What `run` comes to, or why it cannot start.
    pub(crate) async fn run<T>(
        town: &Rc<Town>,
        who: &Resident,
        argv: &[impl AsRef<std::ffi::OsStr>],
        folder: Option<&Path>,
        run: impl AsyncFnOnce(&mut Child, ChildStdout, ChildStdin) -> T,
    ) -> std::io::Result<T> {
        let mut command = command(town, argv)?;
        command.stdin(Stdio::piped()).stdout(Stdio::piped());
        command.stderr(Stdio::piped()).process_group(0).kill_on_drop(true);
        if let Some(folder) = folder {
            command.current_dir(folder);
        }
        let mut child = command.spawn()?;
        let group = Group::of(town, &child);
        let (out, inp, err) = (child.stdout.take(), child.stdin.take(), child.stderr.take());
        let mut stderr = errors(town, who, err.expect("piped"));
        let ran = run(&mut child, out.expect("piped"), inp.expect("piped")).await;
        // all it started ends with it, so its stderr closes, and the last of it is heard; but one
        // that left the group, as `setsid` does, may hold it open: then a moment is all it is given
        drop(group);
        stderr.last().await;
        Ok(ran)
    }
}

/// A task that ends with whatever holds it, however it ends: a reopen's abort too.
pub(crate) struct Task(pub(crate) tokio::task::JoinHandle<()>);

impl Task {
    /// waited for to its end
    pub(crate) async fn ended(&mut self) {
        (&mut self.0).await.ok();
    }

    /// waited for to its end, but `LAST` at most: what it reads may be held open by one outside
    /// all that ended, as a program that left its group holds its stderr
    pub(crate) async fn last(&mut self) {
        let _ = tokio::time::timeout(LAST, self.ended()).await;
    }
}

/// How long the last of what a run said is waited for, once all it started has ended.
pub(crate) const LAST: Duration = Duration::from_secs(1);

impl Drop for Task {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// A task that ends as this is dropped, though another holds it: a join's run, held in
/// `Town::tasks` too, so it ends as they do.
pub(crate) struct Ends(pub(crate) tokio::task::AbortHandle);

impl Drop for Ends {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// A program's stderr, each line a `town/error` from it, as far as `cut` keeps it: read aside, so
/// all it says is heard, to the last once it and all it started end (`Task::ended`).
pub(crate) fn errors(
    town: &Rc<Town>,
    who: &Resident,
    err: impl tokio::io::AsyncRead + Unpin + 'static,
) -> Task {
    let (town, who) = (town.clone(), who.clone());
    Task(tokio::task::spawn_local(async move {
        let mut voice = crate::voice::Voice::new(err);
        while let Some(line) = voice.next().await {
            town.error(&who, None, crate::town::cut(&line).into_owned());
        }
    }))
}

/// Whether a file runs: one, that may be executed.
pub(crate) fn runs(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.metadata().is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

/// `argv` as a process in the town's world alone: its vars, and its first word found on its `PATH`
/// unless it is a path. Every process town starts begins here.
pub(crate) fn command(
    town: &Town,
    argv: &[impl AsRef<std::ffi::OsStr>],
) -> std::io::Result<tokio::process::Command> {
    let mut command = tokio::process::Command::new(found(town, argv[0].as_ref())?);
    command.args(&argv[1..]).env_clear().envs(&town.world);
    Ok(command)
}

/// Where `name` is in the town's world: itself, if it is a path; else the first runnable file of
/// that name in a folder on the world's `PATH`.
fn found(town: &Town, name: &std::ffi::OsStr) -> std::io::Result<PathBuf> {
    if name.as_encoded_bytes().contains(&b'/') {
        return Ok(name.into());
    }
    let path = town.world.get(std::ffi::OsStr::new("PATH"));
    let found = path
        .into_iter()
        .flat_map(std::env::split_paths)
        .map(|dir| dir.join(name))
        .find(|p| runs(p));
    let missing = || format!("{} is not on the world's PATH", name.to_string_lossy());
    found.ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, missing()))
}

impl Drop for Group {
    fn drop(&mut self) {
        // one the town has ended already is not ended twice
        if self.0.groups.borrow_mut().remove(&self.1) {
            unix::end_group(self.1);
        }
    }
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
    town.end_groups();
    std::process::exit(128 + sig.as_raw_value()) // as a shell says a signal ended it
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::body::Body;
    use crate::residents::inside::{end, script, speaking};
    use crate::residents::open;
    use crate::testing::{an_error, guard, run, scratch, until, upto};
    use crate::word::{Tense, Word};
    use mlua::Lua;

    // a program speaks for the residents under it: it says one is there, then words by it,
    // each checked as that one's
    #[test]
    fn a_program_speaks_for_those_under_it() {
        run(async {
            let dir = scratch();
            let host = r#"echo '{"kinds":["ping"]}'
                echo '{"kind":"town/residents/sub","body":{"own":["subs"]}}'
                echo '{"kind":"subs","tense":"past","body":"hi","by":"sub"}'
                echo '{"kind":"subs","tense":"past","body":"not mine"}'
                exec sleep 60"#;
            let host = script(&dir, "host", host);
            let town = Town::with_log(dir.log());
            let mut words = town.words.listen();
            open(&Lua::new(), &town, &[host]);
            let hi = until(&mut words, |w| w.kind == "subs").await;
            assert_eq!(hi.body, Body::of("hi"), "by the one it speaks for");
            let refused = until(&mut words, |w| w.kind == "town/error").await;
            assert_eq!(refused.body.value()["from"], "host", "not its own");
            assert_eq!(refused.body.value()["heard"], "subs");
            town.reopen();
        });
    }

    // a program waits for its language: one that ends as it waits is heard to end; one may speak
    // its own; and what one says before its language is, JSON but no word, is heard in it after
    #[test]
    fn a_program_waits_for_its_language() {
        run(async {
            let dir = scratch();
            // what it starts holds its output open: it has ended all the same
            let typo = script(&dir, "typo", r#"echo '{"speaks":"wrds"}'; sleep 60 & exit 3"#);
            let town = Town::with_log(dir.log());
            let mut words = town.words.listen();
            open(&Lua::new(), &town, &[typo]);
            let from = |w: &Word| w.kind == "town/error" && w.body.value()["from"] == "typo";
            let waits = until(&mut words, from).await.body.value()["message"].clone();
            assert_eq!(waits, "no language is called wrds yet: it waits for one");
            let ended = until(&mut words, from).await.body.value()["message"].clone();
            assert_eq!(ended, "ended: exit status: 3");
            // one that is its own language, and speaks it
            let grammar = r#"{"seq":[{"part":["kind",{"upto":"\n"}]}]}"#;
            let first =
                r#"{"speaks":"bare","languages":{"bare":GRAMMAR}}"#.replace("GRAMMAR", grammar);
            let first = format!("printf '%s\\n' '{first}'");
            let says = format!("{first}\necho hello; exec sleep 60");
            open(&Lua::new(), &town, &[script(&dir, "bare", &says)]);
            until(&mut words, |w| w.kind == "hello").await;
            // one whose language comes after it: what it says first is held for it
            let early = r#"echo '{"speaks":"late"}'; echo 123; echo second; exec sleep 60"#;
            open(&Lua::new(), &town, &[script(&dir, "early", early)]);
            let from = |w: &Word| w.kind == "town/error" && w.body.value()["from"] == "early";
            let waits = until(&mut words, from).await.body.value()["message"].clone();
            assert_eq!(waits, "no language is called late yet: it waits for one");
            speaking(&town, &dir, "late", r#"seq{ { kind = upto("\n") } }"#);
            let heard = upto(&mut words, |w| w.kind == "second");
            let heard: Vec<_> = heard.await.into_iter().map(|w| w.kind).collect();
            assert!(heard[0].starts_with("town/residents/"), "the language, first: {heard:?}");
            assert_eq!(heard[1..], ["123", "second"], "JSON, but no word: {heard:?}");
            town.reopen();
        });
    }

    // a program runs once: one that fails says why, on stderr and in how it ended; one that ends
    // well says nothing; one that never says what it is, or ends before it does, says so. None runs
    // again. What it owns is its own only while it runs
    #[test]
    fn a_program_runs_once() {
        run(async {
            let dir = scratch();
            let began = r#"echo '{}'; echo '{"kind":"began","tense":"past"}'"#;
            let long = "x".repeat(3024);
            let broken = format!("{began}; echo oops >&2; echo {long} >&2; exit 3");
            let broken = script(&dir, "broken", &broken);
            let ending = r#"echo '{"kinds":["go"],"own":["mine"]}'
                echo '{"kind":"up","tense":"past"}'; read -r _"#;
            let ending = script(&dir, "ending", ending);
            let mute = script(&dir, "mute", r#"echo '{"kind":"began"}'"#);
            let early = script(&dir, "early", "exit 4");
            let town = Town::with_log(dir.log());
            let mut words = town.words.listen();
            open(&Lua::new(), &town, &[broken, ending, mute, early]);
            let mut runs: Vec<_> = town.tasks.borrow_mut().drain(..).map(|(_, t)| t).collect();
            // ending owns what it says it does while it runs, and gives it back as it ends
            let mine = Word::new("mine", Tense::Past, Body::none());
            let mut said = upto(&mut words, |w| w.kind == "up").await;
            assert!(town.may(&mine, &town.who("square", true)).is_err(), "its own, while it runs");
            town.talk(Word::new("go", Tense::Past, Body::none()));
            // every run ends, and its task with it: all they will ever say has been said
            for run in &mut runs {
                guard(run).await.unwrap();
            }
            assert!(town.may(&mine, &town.who("square", true)).is_ok(), "given back");
            said.extend(std::iter::from_fn(|| words.try_recv().ok()));
            let from = |from: &str, is: &str| {
                said.iter()
                    .filter(|w| w.kind == "town/error" && w.body.value()["from"] == from)
                    .filter(|w| w.body.value()["message"].as_str().unwrap().starts_with(is))
                    .count()
            };
            assert_eq!(said.iter().filter(|w| w.kind == "began").count(), 1);
            assert_eq!(from("broken", ""), 3, "its stderr, and how it ended");
            assert_eq!(from("broken", "oops"), 1);
            assert_eq!(from("broken", "ended: exit status: 3"), 1);
            assert_eq!(from("broken", &format!("{}… and 2000 more bytes", "x".repeat(1024))), 1);
            assert_eq!(from("ending", ""), 0);
            assert_eq!(from("mute", ""), 1);
            assert_eq!(from("mute", "its first line must say what it is"), 1);
            assert_eq!(from("early", ""), 1);
            assert_eq!(from("early", "ended before it said what it is: exit status: 4"), 1);
        });
    }

    // a program lives until it ends: one that stops saying, its output closed, is still told
    #[test]
    fn a_program_that_stops_saying_is_still_told() {
        run(async {
            let dir = scratch();
            // its output closed, it says so aside, then says aside what it is told
            let quiet = r#"echo '{"kinds":["ping"]}'; exec >&-; echo ready >&2
                while read -r w; do echo "$w" >&2; done"#;
            let quiet = script(&dir, "quiet", quiet);
            let town = Town::with_log(dir.log());
            let mut words = town.words.listen();
            open(&Lua::new(), &town, &[quiet]);
            until(&mut words, an_error("ready")).await;
            // long enough that its silence is heard first
            tokio::time::sleep(Duration::from_millis(100)).await;
            town.talk(Word::new("ping", Tense::Past, Body::none()));
            until(&mut words, an_error("ping")).await;
            town.reopen();
        });
    }

    // a program never hears its own errors, though it hears `town/error`: one that says aside all
    // it hears would else say its error again without end
    #[test]
    fn a_program_does_not_hear_itself_fail() {
        run(async {
            let dir = scratch();
            let loud =
                r#"echo '{"kinds":["/","town/error"]}'; while read -r w; do echo "$w" >&2; done"#;
            let loud = script(&dir, "loud", loud);
            let town = Town::with_log(dir.log());
            let mut words = town.words.listen();
            open(&Lua::new(), &town, &[loud]);
            let failed = |w: &Word| w.kind == "town/error" && w.body.value()["from"] == "loud";
            let message = |w: Word| w.body.value()["message"].as_str().unwrap().to_string();
            let itself = message(until(&mut words, failed).await);
            assert!(itself.contains("town/residents/"), "it fails first at what it is: {itself}");
            let now = message(until(&mut words, failed).await);
            assert!(now.contains(r#""kind":"""#), "then at what follows is now: {now}");
            town.talk(Word::new("pong", Tense::Past, Body::none()));
            let next = message(until(&mut words, failed).await);
            assert!(next.contains("pong"), "then at pong, not at its own error: {next}");
            town.reopen();
        });
    }

    // a program that does not read has only so many words waiting: what is dropped is said, once
    #[test]
    fn a_program_that_does_not_read_is_said_once() {
        run(async {
            let dir = scratch();
            let deaf =
                r#"echo '{"kinds":["ping"]}'; echo '{"kind":"up","tense":"past"}'; exec sleep 60"#;
            let deaf = script(&dir, "deaf", deaf);
            let town = Town::with_log(dir.log());
            let mut words = town.words.listen();
            open(&Lua::new(), &town, &[deaf]);
            until(&mut words, |w| w.kind == "up").await;
            let dropped = |w: &Word| w.body.value()["message"] == "is not reading: words dropped";
            let (mut said, mut pings) = (0, 0);
            // pings until the pipe and the queue are full and it is said; then as many again
            while said == 0 || pings < 20_000 {
                town.talk(Word::new("ping", Tense::Past, Body::none()));
                pings += 1;
                tokio::task::yield_now().await;
                said += std::iter::from_fn(|| words.try_recv().ok()).filter(dropped).count();
                assert!(pings < 100_000, "never said");
            }
            assert_eq!(said, 1);
            town.reopen();
        });
    }

    // all a program starts ends with it: a child it left running, as a reopen ends the run
    #[test]
    fn a_programs_processes_end_with_it() {
        run(async {
            let dir = scratch();
            let parent = r#"sleep 60 & echo $! > child
                echo '{}'; echo '{"kind":"up","tense":"past"}'; exec cat"#;
            let parent = script(&dir, "parent", parent);
            let town = Town::with_log(dir.log());
            let mut words = town.words.listen();
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
        });
    }

    // the last of what a run said is heard, but one holding its stderr open from outside all it
    // started holds town only a moment: the pipe here is held as such a one would
    #[test]
    fn errors_held_open_end_in_a_moment() {
        run(async {
            use std::io::Write;
            let dir = scratch();
            let town = Town::with_log(dir.log());
            let who = town.who("held", false);
            let mut words = town.words.listen();
            let (r, mut held) = std::io::pipe().unwrap();
            held.write_all(b"last\n").unwrap();
            let r = tokio::net::unix::pipe::Receiver::from_owned_fd(r.into()).unwrap();
            let mut errors = errors(&town, &who, r);
            let began = tokio::time::Instant::now();
            guard(errors.last()).await;
            assert!(began.elapsed() < LAST * 2, "a moment, not until it closes");
            until(&mut words, an_error("last")).await;
            drop(held);
        });
    }
}
