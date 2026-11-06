//! `mirror`, joined, as a resident joins it: what a path contains, said as facts of a kind.
//!   cargo test --test mirror

use std::path::Path;

use serde_json::json;
use town::body::Body;
use town::parts::Parts;
use town::town::{Listening, Town};
use town::word::{Tense, Word};

use town::testing::{self, resident, scratch, until, upto};

/// `path` mirrored under `kind`, as a resident joins `mirror`
fn mirror(town: &Town, kind: &str, path: &Path) {
    let argv = json!([env!("CARGO_BIN_EXE_mirror"), path]);
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

/// a file as `mirror` says it, as it is now: where it is, when it last changed by the system's own
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

/// `kind` said as `path` is now. A write makes a file, then fills it, and the mirror may say it
/// between: what is waited for is the file as written.
async fn written(words: &mut Listening, kind: &str, path: &Path) {
    let now = file(path);
    until(words, |w| w.kind == kind && w.body == now).await;
}

/// whether `kind` is the mirror's own: no one else may say it
fn own(town: &Town, kind: &str) -> bool {
    town.may(&Word::new(kind, Tense::Present, Body::none()), &town.who("square", true)).is_err()
}

// a file is said as where it is and when it changed, the mirror's own to say; gone, as nothing;
// back, again
#[test]
fn a_file_is_said_as_where_it_is() {
    run(async |town, dir| {
        let colors = dir.join("colors");
        std::fs::write(&colors, "bg=#000").unwrap();
        let mut words = town.words.listen();
        mirror(town, "colors", &colors);
        let fact = |w: &Word| w.kind == "colors" && w.tense == Tense::Present;
        assert_eq!(until(&mut words, fact).await.body, file(&colors));
        assert!(own(town, "colors"), "its own");
        std::fs::remove_file(&colors).unwrap();
        assert!(until(&mut words, fact).await.body.is_none());
        std::fs::write(&colors, "bg=#fff").unwrap();
        written(&mut words, "colors", &colors).await;
    });
}

// what town has already, said again, is no word: a town opened again hears only what changed
#[test]
fn what_town_has_is_not_said_again() {
    run(async |town, dir| {
        let colors = dir.join("colors");
        std::fs::write(&colors, "bg=#000").unwrap();
        town.talk(Word::new("colors", Tense::Present, file(&colors)));
        let mut words = town.words.listen();
        mirror(town, "colors", &colors);
        until(&mut words, resident("colors", Tense::Past)).await;
        let was = file(&colors);
        std::fs::write(&colors, "bg=#111").unwrap();
        let now = file(&colors);
        let heard = upto(&mut words, |w| w.kind == "colors" && w.body == now).await;
        // a write empties the file, then fills it, and the mirror may say it between: what is not
        // said is what town had
        let again = heard.iter().filter(|w| w.kind == "colors" && w.body == was);
        assert_eq!(again.count(), 0, "not again: the change, not what town had");
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
        mirror(town, "files", &folder);
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

// a link is followed to what it leads to, and mirrored there: a folder out of the path, said
// under the link and heard as it changes; a link back up, read once; and one that leads nowhere
// yet, heard as what it leads to is made, as a program built beside a town is
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
        mirror(town, "files", &folder);
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
        mirror(town, "colors", &colors);
        until(&mut words, resident("colors", Tense::Past)).await;
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
        mirror(town, "colors", &colors);
        until(&mut words, resident("colors", Tense::Past)).await;
        std::fs::write(&colors, "bg=#000").unwrap();
        written(&mut words, "colors", &colors).await;
    });
}

// each file is said once, by one name, at first and as it changes: a link to a folder in the path
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
        mirror(town, "files", &folder);
        until(&mut words, resident("files", Tense::Past)).await;
        let under = || town.facts("files/").into_keys().collect::<Vec<String>>();
        assert_eq!(under(), ["files/b/y", "files/real/x"], "at first");
        std::fs::write(folder.join("real/x"), "12").unwrap();
        let x = file(&folder.join("real/x"));
        until(&mut words, |w| w.kind == "files/real/x" && w.body == x).await;
        std::fs::write(outside.join("y"), "12").unwrap();
        let y = file(&folder.join("b/y"));
        until(&mut words, |w| w.kind == "files/b/y" && w.body == y).await;
        // a file after it, heard: what the change of y would say, it has said
        std::fs::write(folder.join("zz"), "1").unwrap();
        written(&mut words, "files/zz", &folder.join("zz")).await;
        assert_eq!(under(), ["files/b/y", "files/real/x", "files/zz"], "as it changes");
    });
}

// a link that leads up, to a folder the path is in, leads back into the path: a file in it is
// said where it truly is, and not again through the link. A change to `zz`, after, is said after
// any other the first change would make
#[test]
fn a_link_up_says_a_file_once() {
    run(async |town, dir| {
        let folder = dir.join("up/files");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("a"), "1").unwrap();
        std::fs::write(folder.join("zz"), "1").unwrap();
        std::os::unix::fs::symlink(dir.join("up"), folder.join("up")).unwrap();
        let mut words = town.words.listen();
        mirror(town, "files", &folder);
        until(&mut words, resident("files", Tense::Past)).await;
        let under = || town.facts("files/").into_keys().collect::<Vec<String>>();
        assert_eq!(under(), ["files/a", "files/zz"], "at first");
        std::fs::write(folder.join("a"), "12").unwrap();
        written(&mut words, "files/a", &folder.join("a")).await;
        std::fs::write(folder.join("zz"), "12").unwrap();
        written(&mut words, "files/zz", &folder.join("zz")).await;
        assert_eq!(under(), ["files/a", "files/zz"], "as it changes");
    });
}
