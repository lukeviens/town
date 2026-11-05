//! `watch`, joined, as a resident joins it: what a path holds, said as facts of a kind; and a
//! town that reloads with it.
//!   cargo test --test watch

use std::path::Path;

use serde_json::json;
use town::body::Body;
use town::parts::Parts;
use town::town::{Listening, Town};
use town::word::{Tense, Word};

use town::testing::{self, guard, scratch, until, upto};

/// `path` watched under `kind`, as a resident joins `watch`
fn watch(town: &Town, kind: &str, path: &Path) {
    let argv = json!([env!("CARGO_BIN_EXE_watch"), path]);
    let join = json!({ "as": kind, "argv": argv });
    town.talk(Word::new("town/join", Tense::Future, Body::of(join)));
}

/// `test`, in a town of its own, with a folder of its own
fn run(test: impl AsyncFnOnce(&std::rc::Rc<Town>, &Path)) {
    let dir = scratch();
    testing::run(async {
        let town = Town::with_log(dir.log());
        test(&town, &dir).await;
        town.reopen();
    });
}

/// a file as `watch` says it, as it is now: where it is, when it last changed by the system's own
/// clock for it, in ns, and how big it is
fn file(path: &Path) -> Body {
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::metadata(path).unwrap();
    let changed = meta.ctime() * 1_000_000_000 + meta.ctime_nsec();
    Body::of(Parts::from(json!({ "path": path, "changed": changed, "size": meta.len() })))
}

/// the next word said at `kind`, its body
async fn said(words: &mut Listening, kind: &str) -> Body {
    until(words, |w| w.kind == kind).await.body
}

/// `kind` said as `path` is now. A write makes a file, then fills it, and the watch may say it
/// between: what is waited for is the file as written.
async fn written(words: &mut Listening, kind: &str, path: &Path) {
    let now = file(path);
    until(words, |w| w.kind == kind && w.body == now).await;
}

/// whether `kind` is the watch's own: no one else may say it
fn own(town: &Town, kind: &str) -> bool {
    town.may(&Word::new(kind, Tense::Present, Body::none()), &town.who("square", true)).is_err()
}

// a file is said as where it is and when it changed, the watch's own to say; gone, as nothing;
// back, again
#[test]
fn a_file_is_said_as_where_it_is() {
    run(async |town, dir| {
        let colors = dir.join("colors");
        std::fs::write(&colors, "bg=#000").unwrap();
        let mut words = town.words.listen();
        watch(town, "colors", &colors);
        let fact = |w: &Word| w.kind == "colors" && w.tense == Tense::Present;
        assert_eq!(until(&mut words, fact).await.body, file(&colors));
        assert!(own(town, "colors"), "its own");
        std::fs::remove_file(&colors).unwrap();
        assert!(until(&mut words, fact).await.body.is_none());
        std::fs::write(&colors, "bg=#fff").unwrap();
        written(&mut words, "colors", &colors).await;
    });
}

// what town has already is not said again: a town opened again hears only what changed
#[test]
fn what_town_has_is_not_said_again() {
    run(async |town, dir| {
        let colors = dir.join("colors");
        std::fs::write(&colors, "bg=#000").unwrap();
        town.talk(Word::new("colors", Tense::Present, file(&colors)));
        let mut words = town.words.listen();
        watch(town, "colors", &colors);
        let fact = |w: &Word| w.kind == "colors" && w.tense == Tense::Present;
        let joined = upto(&mut words, |w| w.kind == "colors" && w.tense == Tense::Past).await;
        assert!(!joined.iter().any(fact), "not again");
        std::fs::write(&colors, "bg=#111").unwrap();
        written(&mut words, "colors", &colors).await; // the change, not what town had
    });
}

// a folder is said as each file in it, however deep, any bytes, under its kind: again as one
// changes, as nothing once it is gone; what town had of it, no longer there, is let go
#[test]
fn a_folder_is_said_as_each_file_in_it() {
    run(async |town, dir| {
        let folder = dir.join("files");
        std::fs::create_dir_all(folder.join("sub")).unwrap();
        std::fs::write(folder.join("a"), "1").unwrap();
        // a picture, say
        std::fs::write(folder.join("sub/b"), [0xff, 0xd8]).unwrap();
        // what is hidden in it is not of it: never said
        std::fs::create_dir_all(folder.join(".git")).unwrap();
        std::fs::write(folder.join(".git/HEAD"), "ref").unwrap();
        std::fs::write(folder.join(".DS_Store"), "x").unwrap();
        town.talk(Word::new("files/old", Tense::Present, Body::of("x")));
        let mut words = town.words.listen();
        watch(town, "files", &folder);
        assert_eq!(said(&mut words, "files/a").await, file(&folder.join("a")));
        assert_eq!(said(&mut words, "files/sub/b").await, file(&folder.join("sub/b")));
        assert!(said(&mut words, "files/old").await.is_none(), "let go");
        assert!(own(town, "files/a"), "its own");
        std::fs::write(folder.join("a"), "11").unwrap();
        written(&mut words, "files/a", &folder.join("a")).await;
        std::fs::remove_file(folder.join("sub/b")).unwrap();
        assert!(said(&mut words, "files/sub/b").await.is_none(), "gone");
        // a folder come whole and gone whole: each file in it, then each as nothing
        let aside = dir.join("aside");
        std::fs::create_dir_all(&aside).unwrap();
        std::fs::write(aside.join("c"), "3").unwrap();
        std::fs::rename(&aside, folder.join("more")).unwrap();
        assert_eq!(said(&mut words, "files/more/c").await, file(&folder.join("more/c")));
        std::fs::rename(folder.join("more"), dir.join("away")).unwrap();
        assert!(said(&mut words, "files/more/c").await.is_none());
        let facts: Vec<String> = town.facts("files/").into_keys().collect();
        assert_eq!(facts, ["files/a"], "nothing hidden, nothing gone");
    });
}

// a link is followed to what it leads to, and watched there: a folder out of the watch, said under
// the link and heard as it changes; a link back up, read once; and one that leads nowhere yet,
// heard as what it leads to is made, as a program built beside a town is
#[test]
fn a_link_is_followed_where_it_leads() {
    run(async |town, dir| {
        let (folder, outside) = (dir.join("files"), dir.join("outside"));
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("colors"), "bg=#000").unwrap();
        std::os::unix::fs::symlink(&outside, folder.join("theme")).unwrap();
        std::os::unix::fs::symlink(&folder, folder.join("up")).unwrap();
        std::os::unix::fs::symlink(dir.join("build/out"), folder.join("later")).unwrap();
        let mut words = town.words.listen();
        watch(town, "files", &folder);
        written(&mut words, "files/theme/colors", &folder.join("theme/colors")).await;
        std::fs::write(outside.join("colors"), "bg=#111").unwrap();
        written(&mut words, "files/theme/colors", &folder.join("theme/colors")).await;
        let under: Vec<String> = town.facts("files/").into_keys().collect();
        assert_eq!(under, ["files/theme/colors"], "the link back up, read once");
        std::fs::create_dir_all(dir.join("build")).unwrap();
        std::fs::write(dir.join("build/out"), "built").unwrap();
        written(&mut words, "files/later", &folder.join("later")).await;
    });
}

// a file whose folder is not there yet is heard once both are made
#[test]
fn a_path_not_there_yet_is_heard_as_it_comes() {
    run(async |town, dir| {
        let colors = dir.join("theme").join("colors");
        let mut words = town.words.listen();
        watch(town, "colors", &colors);
        until(&mut words, |w| w.kind == "colors" && w.tense == Tense::Past).await; // joined
        std::fs::create_dir(dir.join("theme")).unwrap();
        std::fs::write(&colors, "bg=#000").unwrap();
        written(&mut words, "colors", &colors).await;
    });
}

// a file not there yet, in a folder named through a link, is heard as it comes
#[test]
fn a_file_named_through_a_link_is_heard_as_it_comes() {
    run(async |town, dir| {
        std::fs::create_dir_all(dir.join("real")).unwrap();
        std::os::unix::fs::symlink(dir.join("real"), dir.join("linked")).unwrap();
        let colors = dir.join("linked/colors");
        let mut words = town.words.listen();
        watch(town, "colors", &colors);
        until(&mut words, |w| w.kind == "colors" && w.tense == Tense::Past).await; // joined
        std::fs::write(&colors, "bg=#000").unwrap();
        written(&mut words, "colors", &colors).await;
    });
}

// each file is said once, by one name, at first and as it changes: a link to a folder in the watch
// is not followed, so a file there is said where it truly is; a folder out of it two links lead
// to, under the first
#[test]
fn a_file_is_said_once_where_it_is() {
    run(async |town, dir| {
        let (folder, outside) = (dir.join("files"), dir.join("outside"));
        std::fs::create_dir_all(folder.join("real")).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(folder.join("real/x"), "1").unwrap();
        std::fs::write(outside.join("y"), "1").unwrap();
        std::os::unix::fs::symlink(folder.join("real"), folder.join("a")).unwrap();
        std::os::unix::fs::symlink(&outside, folder.join("b")).unwrap();
        std::os::unix::fs::symlink(&outside, folder.join("c")).unwrap();
        let mut words = town.words.listen();
        watch(town, "files", &folder);
        until(&mut words, |w| w.kind == "files" && w.tense == Tense::Past).await; // joined
        let under = || town.facts("files/").into_keys().collect::<Vec<String>>();
        assert_eq!(under(), ["files/b/y", "files/real/x"], "at first");
        std::fs::write(folder.join("real/x"), "12").unwrap();
        let x = file(&folder.join("real/x"));
        until(&mut words, |w| w.kind == "files/real/x" && w.body == x).await;
        std::fs::write(outside.join("y"), "12").unwrap();
        let y = file(&folder.join("b/y"));
        until(&mut words, |w| w.kind == "files/b/y" && w.body == y).await;
        assert_eq!(under(), ["files/b/y", "files/real/x"], "as it changes");
    });
}

/// a program in `dir` that says a word of `kind`, and listens
fn made(dir: &Path, kind: &str) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join(kind);
    let says = format!("#!/bin/sh\necho '{{}}'; echo '{{\"kind\":\"{kind}\"}}'; exec cat\n");
    std::fs::write(&path, says).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

// a town that reloads, with town's own `reload`, opens a resident again as it is saved, proven
// each time by a word only the one saved says: saved; the folder replaced whole; a program built
// beside the town and linked in, built again; one linked before it is built, once built
#[test]
fn a_saved_resident_opens_the_town_again() {
    let dir = scratch();
    let (top, aside) = (dir.join("town"), dir.join("aside"));
    let reload = include_str!("../residents/reload.lua");
    // reload is a Lua file: `lua`, built, linked in, speaks for it
    let reloads = |at: &Path| {
        std::fs::write(at.join("reload.lua"), reload).unwrap();
        std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_lua"), at.join("lua")).unwrap();
    };
    std::fs::create_dir_all(&top).unwrap();
    reloads(&top);
    made(&top, "first");
    testing::run(async {
        // its world gives town's watch at `WATCH`
        let world = testing::world_with(Path::new(env!("CARGO_BIN_EXE_watch")));
        let town = Town::in_world(dir.log(), world);
        let mut words = town.words.listen();
        town::residents::keep(&town, vec![top.clone()]);
        // the word; and, when `reload` itself comes again, its watch begun, now, so the next save
        // is heard: a save of any other resident leaves it watching
        let watch = format!("reload{}", top.display());
        let mut opened = async |kind: &str, again: bool| {
            let (mut said, mut now) = (false, !again);
            guard(async {
                while !(said && now) {
                    let w = words.recv().await.expect("the town closed");
                    said |= w.kind == kind;
                    now |= w.kind == watch && w.tense == Tense::Past && w.body.is_none();
                }
            })
            .await
        };
        opened("first", true).await;
        made(&top, "second");
        opened("second", false).await;
        // the folder itself, replaced whole
        std::fs::create_dir_all(&aside).unwrap();
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
        made(&top, "linked"); // heard once the town has seen the link, and watches for it
        opened("linked", false).await;
        std::fs::create_dir_all(&release).unwrap();
        std::fs::rename(made(&release, "built-later"), release.join("bot")).unwrap();
        opened("built-later", false).await;
        town.reopen();
    });
}

// a town opened with a path and one inside it watches the outer alone: each file is said by one
// watch, so none is refused, and a save inside the inner opens the town again
#[test]
fn a_path_inside_another_is_watched_once() {
    let dir = scratch();
    let (top, sub) = (dir.join("town"), dir.join("town/sub"));
    std::fs::create_dir_all(&sub).unwrap();
    std::fs::write(top.join("reload.lua"), include_str!("../residents/reload.lua")).unwrap();
    std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_lua"), top.join("lua")).unwrap();
    made(&sub, "inner");
    testing::run(async {
        let world = testing::world_with(Path::new(env!("CARGO_BIN_EXE_watch")));
        let town = Town::in_world(dir.log(), world);
        let mut words = town.words.listen();
        let mut errors = town.words.hear(|_, w| w.kind == town::word::ERROR, None);
        town::residents::keep(&town, vec![top.clone(), sub.clone()]);
        let watch = format!("reload{}", top.display());
        let begun = |w: &Word| w.kind == watch && w.tense == Tense::Past && w.body.is_none();
        until(&mut words, begun).await;
        made(&sub, "saved");
        until(&mut words, |w| w.kind == "saved").await;
        let refused = std::iter::from_fn(|| errors.try_recv().ok())
            .filter_map(|e| e.body.part("message").and_then(Parts::text).map(String::from))
            .find(|m| m.contains("another owns it"));
        assert_eq!(refused, None, "each file said by one watch");
        town.reopen();
    });
}
