//! A town that reloads, with town's own `reload`: a resident saved opens the town again, and one
//! not changed runs once.
//!   cargo test --test reload

use std::path::{Path, PathBuf};
use std::rc::Rc;

use town::parts::Parts;
use town::town::{Listening, Town};
use town::word::Tense;

use town::testing::{self, probe, resident, scratch, script, settled, until, Scratch};

/// a program in `dir` that says a word of `kind`, and listens
fn made(dir: &Path, kind: &str) -> PathBuf {
    let says = format!(r#"echo '{{}}'; echo '{{"kind":"{kind}","tense":"past"}}'; exec cat"#);
    script(dir, kind, &says)
}

/// `reload` in `at`, a Lua file that `lua`, built and linked in, speaks for; and the probe beside
/// it, so a test may settle
fn reloads(at: &Path) {
    std::fs::create_dir_all(at).unwrap();
    std::fs::write(at.join("reload.lua"), include_str!("../residents/reload.lua")).unwrap();
    std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_lua"), at.join("lua")).unwrap();
    probe(at);
}

/// a town of a test's own, whose world gives `mirror` on its PATH, and what it says, heard from
/// before it opens
fn town(dir: &Scratch) -> (Rc<Town>, Listening) {
    let world = testing::world_finding(Path::new(env!("CARGO_BIN_EXE_mirror")));
    let town = Town::in_world(dir.log(), world);
    let words = town.words.listen();
    (town, words)
}

/// the place of the mirror `reload` joins for `top`
fn mirrors(top: &Path) -> String {
    format!("reload{}", top.display())
}

// a town that reloads opens a resident again as it is saved, proven each time by a word only the
// one saved says: saved; the folder replaced whole; a program built beside the town and linked in,
// built again; one linked before it is built, once built
#[test]
fn a_saved_resident_opens_the_town_again() {
    let dir = scratch();
    let (top, aside) = (dir.join("town"), dir.join("aside"));
    reloads(&top);
    made(&top, "first");
    testing::run(async {
        let (town, mut words) = town(&dir);
        town::residents::keep(&town, vec![top.clone()]);
        // the word; and, when `reload` itself comes again, its mirror begun, now, so the next save
        // is heard: a save of any other resident leaves its mirror as it is
        let place = mirrors(&top);
        let mut opened = async |kind: &str, again: bool| {
            let (mut said, mut now) = (false, !again);
            let opens = async {
                while !(said && now) {
                    let w = words.recv().await.expect("the town closed");
                    said |= w.kind == kind;
                    now |= resident(&place, Tense::Past)(&w);
                }
            };
            let came = tokio::time::timeout(testing::GUARD, opens).await;
            came.unwrap_or_else(|_| panic!("never opened for `{kind}`: said {said}, now {now}"));
        };
        opened("first", true).await;
        made(&top, "second");
        opened("second", false).await;
        // the folder itself, replaced whole
        reloads(&aside);
        made(&aside, "third");
        std::fs::remove_dir_all(&top).unwrap();
        std::fs::rename(&aside, &top).unwrap();
        opened("third", true).await;
        // a program built beside the town, linked into it: built again, it runs again
        let beside = dir.join("beside");
        std::fs::create_dir_all(&beside).unwrap();
        std::os::unix::fs::symlink(made(&beside, "built"), top.join("bot")).unwrap();
        opened("built", false).await;
        std::fs::rename(made(&beside, "rebuilt"), beside.join("built")).unwrap();
        opened("rebuilt", false).await;
        // linked before it is ever built, as after a clean: once built, it runs
        let release = dir.join("later/target/release");
        std::os::unix::fs::symlink(release.join("bot"), top.join("later")).unwrap();
        made(&top, "linked"); // heard once the town has seen the link, and mirrors it
        opened("linked", false).await;
        std::fs::create_dir_all(&release).unwrap();
        std::fs::rename(made(&release, "built-later"), release.join("bot")).unwrap();
        opened("built-later", false).await;
        town.reopen();
    });
}

// a town opened with a path and one inside it mirrors the outer alone: each file is said by one
// mirror, so none is refused, and a save inside the inner opens the town again
#[test]
fn a_path_inside_another_is_mirrored_once() {
    let dir = scratch();
    let (top, sub) = (dir.join("town"), dir.join("town/sub"));
    reloads(&top);
    std::fs::create_dir_all(&sub).unwrap();
    made(&sub, "inner");
    testing::run(async {
        let (town, mut words) = town(&dir);
        town::residents::keep(&town, vec![top.clone(), sub.clone()]);
        until(&mut words, resident(&mirrors(&top), Tense::Past)).await;
        made(&sub, "saved");
        until(&mut words, |w| w.kind == "saved").await;
        // all lua asked is done: the joins it made are all there are
        settled(&town, &mut words).await;
        let joined: Vec<String> = town.joins.borrow().keys().cloned().collect();
        assert_eq!(joined, [mirrors(&top)], "each file said by one mirror");
        town.reopen();
    });
}

// a town that reloads runs each program once as it opens: its mirror, as it begins, says every
// file, and reload asks of each, but town opens again only one that is not the version it ran:
// the answer to the ask of `kept` names no file, and it ran once
#[test]
fn a_town_that_reloads_runs_each_program_once() {
    let dir = scratch();
    let top = dir.join("town");
    reloads(&top);
    made(&top, "kept");
    testing::run(async {
        let (town, mut words) = town(&dir);
        let mut kept = town.words.hear(|_, w| w.kind == "kept", None);
        town::residents::keep(&town, vec![top.clone()]);
        let path = top.join("kept");
        let path = path.to_str().unwrap();
        // the answer, and kept's run as the town opened, in either order: then none ran it again
        let (mut answered, mut ran) = (None, 0);
        while answered.is_none() || ran == 0 {
            let w = until(&mut words, |w| {
                w.kind == "kept"
                    || w.kind == "town/open"
                        && w.tense == Tense::Past
                        && w.body.part("path").and_then(Parts::text) == Some(path)
            })
            .await;
            match w.kind.as_str() {
                "kept" => ran += 1,
                _ => answered = Some(w),
            }
        }
        let paths = answered.unwrap().body.part("paths").and_then(Parts::list).map(|l| l.len());
        assert_eq!(paths, Some(0), "as it ran: nothing opened");
        settled(&town, &mut words).await;
        assert_eq!(std::iter::from_fn(|| kept.try_recv().ok()).count(), 1, "once");
        town.reopen();
    });
}
