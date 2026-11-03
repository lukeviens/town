//! A watch: a file or folder town keeps reading for a resident, said as a fact whenever it
//! changes.

use std::path::{Path, PathBuf};
use std::rc::Rc;

use notify::{RecursiveMode, Watcher};
use tokio::sync::mpsc;

use crate::body::Body;
use crate::parts::Parts;
use crate::town::Town;
use crate::word::{Tense, Word};

/// What a resident watches: a path, read as text, or `{ path, as }`, read in a language.
pub(crate) struct Watched {
    path: String,
    language: Option<String>,
}

impl Watched {
    pub(crate) fn of(p: &Parts) -> Result<Watched, String> {
        if let Some(path) = p.text() {
            return Ok(Watched { path: path.into(), language: None });
        }
        let text = |name| p.get(name).and_then(Parts::text).map(String::from);
        match (p, text("path"), text("as")) {
            (Parts::Map(m), Some(path), Some(language)) if m.len() == 2 => {
                Ok(Watched { path, language: Some(language) })
            }
            _ => Err("a watch is a path, or { path, as }".into()),
        }
    }
}

/// watch: a file or folder town keeps reading for a resident. It speaks for the file, not for
/// anyone. A relative path is beside the resident, not wherever the town was started.
pub(crate) fn watch_for(
    town: &Rc<Town>,
    from: &str,
    resident: &Path,
    kind: String,
    watched: Watched,
) -> tokio::task::JoinHandle<()> {
    let Watched { path: file, language } = watched;
    let file = resident.parent().unwrap_or(Path::new("/")).join(file);
    town.hold(&kind);
    tokio::task::spawn_local(watching(
        town.clone(),
        from.to_string(),
        kind,
        file.to_string_lossy().into(),
        language,
    ))
}

/// A watched file, said as a fact of its content: as the town opens, so its resident is set, and
/// then whenever the content changes, since one save can make several events. A file that is gone
/// is said as a fact of nothing. A folder is said as a bare change. Real filesystem events
/// (FSEvents, inotify), never a tick. A save that takes the file away a moment is said as it
/// happened, gone and then back; a resident that would rather not see the gap waits it out itself
/// (`after`).
async fn watching(
    town: Rc<Town>,
    from: String,
    kind: String,
    path: String,
    language: Option<String>,
) {
    let file = PathBuf::from(&path);
    let dir = file.is_dir();
    // a file through its folder: a save that replaces it would end a watch on the file itself
    let (at, name) = match (dir, file.parent(), file.file_name()) {
        (false, Some(folder), Some(name)) => (folder.to_path_buf(), Some(name.to_os_string())),
        _ => (file.clone(), None),
    };
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut watcher = match notify::recommended_watcher(move |e: notify::Result<notify::Event>| {
        let ours = |p: &PathBuf| name.as_ref().is_none_or(|n| p.file_name() == Some(n));
        if e.is_ok_and(|e| e.paths.iter().any(ours)) {
            tx.send(()).ok();
        }
    }) {
        Ok(w) => w,
        Err(e) => return town.error(&from, None, format!("cannot watch {path}: {e}")),
    };
    let deep = if dir { RecursiveMode::Recursive } else { RecursiveMode::NonRecursive };
    if let Err(e) = watcher.watch(&at, deep) {
        return town.error(&from, None, format!("cannot watch {path}: {e}"));
    }
    if dir {
        while rx.recv().await.is_some() {
            town.talk(Word::new(&kind, Tense::Past, Body::none()));
        }
        return;
    }
    // a language a program says may not be said yet
    if let Some(name) = &language {
        if town.language(name, &from).await.is_none() {
            return;
        }
    }
    // what the present already holds was heard in the greeting: said again only if it differs
    let mut said = town.present.borrow().get(&kind).cloned();
    // a file as text, or as the parts its language reads; one unread keeps what it said
    let read = |bytes: Vec<u8>| match &language {
        None => String::from_utf8(bytes).ok().map(Body::of),
        Some(f) => town
            .languages
            .parts(f, &bytes)
            .map_err(|e| town.error(&from, None, format!("{path}: {e}")))
            .ok()
            .map(Body::of),
    };
    loop {
        match std::fs::read(&path) {
            Ok(bytes) => match read(bytes) {
                Some(body) if said.as_ref() != Some(&body) => {
                    said = Some(body.clone());
                    town.talk(Word::new(&kind, Tense::Present, body));
                }
                _ => {}
            },
            // only a file that is not there is gone
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && said.is_some() => {
                said = None;
                town.talk(Word::new(&kind, Tense::Present, Body::none()));
            }
            _ => {}
        }
        if rx.recv().await.is_none() {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lua::install;
    use crate::lua::wire;
    use crate::parts::Parts;
    use crate::residents::inside::{end, script};
    use crate::residents::open;
    use crate::testing::{guard, until};
    use crate::word::Tense;
    use mlua::{Lua, Table};

    // a file watched as a language is heard as its parts, and only when they change; a resident
    // writes parts back as that language, and a program may say a language of its own
    #[test]
    fn a_file_is_heard_as_its_language() {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        tokio::task::LocalSet::new().block_on(&rt, async {
            let dir = std::env::temp_dir().join(format!("town-language-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            let file = dir.join("colors");
            std::fs::write(&file, "# shared\nbg=#000\n").unwrap();
            let lua = format!(
                r##"local file = {file:?}
                return {{ kinds = {{ "colors", "dark" }},
                  watch = {{ colors = {{ path = file, as = "kv" }} }},
                  listen = function(w)
                    if w.kind ~= "dark" then return event("heard", w.body) end
                    post(file, {{ bg = "#111", tint = 0.9 }}, "kv")
                  end }}"##,
                file = file.to_string_lossy()
            );
            std::fs::write(dir.join("r.lua"), lua).unwrap();
            // a program says a language as any word: one line
            let csv = r#"{"name":"csv","grammar":{"many":{"part":["cell",{"upto":","}]}}}"#;
            let csv = format!(r#"{{"kind":"language","tense":"past","body":{csv}}}"#);
            let said = format!("echo '{{\"kinds\":[]}}'\necho '{csv}'\nexec cat >/dev/null");
            let program = script(&dir, "csv", &said);
            let town = Town::with_log(dir.join("past.log"));
            let lua = Lua::new();
            install(&lua, &town).unwrap();
            let mut words = town.words.subscribe();
            let kv = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/languages/kv.lua"));
            open(&lua, &town, &[kv, dir.join("r.lua"), program]);
            let heard = |w: &Word| w.kind == "heard";
            let first = until(&mut words, heard).await.body;
            assert_eq!(first, Parts::from(serde_json::json!({ "bg": "#000" })));
            std::fs::write(&file, "bg=#000\n# a comment changes no part\n").unwrap();
            town.talk(Word::new("dark", Tense::Past, Body::none()));
            let next = until(&mut words, heard).await.body;
            assert_eq!(next, Parts::from(serde_json::json!({ "bg": "#111", "tint": "0.9" })));
            let kept = town.languages.parts("kv", &std::fs::read(&file).unwrap()).unwrap();
            assert_eq!(kept, Parts::from(serde_json::json!({ "bg": "#111", "tint": "0.9" })));
            let read =
                |as_: &str, bytes: &str| lua.load(format!("return parts({as_:?}, {bytes:?})"));
            guard(async {
                while town.languages.parts("csv", b"").is_err() {
                    tokio::task::yield_now().await
                }
            })
            .await;
            let cells = crate::lua::parts(read("csv", "a,b,").eval().unwrap()).unwrap();
            assert_eq!(cells, Parts::from(serde_json::json!([{ "cell": "a" }, { "cell": "b" }])));
            assert!(read("kv", "a=b\n").eval::<Table>().is_ok());
            let none = read("nope", "").eval::<Table>().unwrap_err().to_string();
            assert!(none.contains("no language"));
            end(&town);
            std::fs::remove_dir_all(&dir).ok();
        });
    }

    // a watched file already kept in the present is heard once as the town opens, in the greeting;
    // a save that replaces the file, as `post` does, is still seen, each time
    #[test]
    fn a_kept_file_is_heard_once_and_every_save_after() {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        tokio::task::LocalSet::new().block_on(&rt, async {
            let dir = std::env::temp_dir().join(format!("town-kept-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            let file = dir.join("colors");
            std::fs::write(&file, "bg=#000").unwrap();
            let kept = Word::new("colors", Tense::Present, Body::of("bg=#000"));
            std::fs::write(dir.join("past.log"), crate::connection::line(&kept) + "\n").unwrap();
            let resident = dir.join("r.lua");
            let watch = format!("watch = {{ colors = {:?} }}", file.to_string_lossy());
            let lua = format!(
                r#"return {{ kinds = {{ "colors" }}, {watch},
                  listen = function(w) return event("heard", w.body) end }}"#
            );
            std::fs::write(&resident, lua).unwrap();
            let town = Town::with_log(dir.join("past.log"));
            let lua = Lua::new();
            install(&lua, &town).unwrap();
            let mut words = town.words.subscribe();
            open(&lua, &town, std::slice::from_ref(&resident));
            // each save is heard once, and next: a second hearing of one would come before it
            let mut heard = vec![until(&mut words, |w| w.kind == "heard").await.body];
            for bg in ["bg=#111", "bg=#222", "bg=#333"] {
                std::fs::write(dir.join("colors.writing"), bg).unwrap();
                std::fs::rename(dir.join("colors.writing"), &file).unwrap();
                heard.push(until(&mut words, |w| w.kind == "heard").await.body);
            }
            assert_eq!(heard, ["bg=#000", "bg=#111", "bg=#222", "bg=#333"]);
            end(&town);
            std::fs::remove_dir_all(&dir).ok();
        });
    }

    // a watched file that is gone is said as a fact of nothing, and said again when it is back
    #[test]
    fn a_gone_file_is_said_as_nothing() {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        tokio::task::LocalSet::new().block_on(&rt, async {
            let dir = std::env::temp_dir().join(format!("town-gone-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            let file = dir.join("colors");
            std::fs::write(&file, "bg=#000").unwrap();
            let resident = dir.join("r.lua");
            std::fs::write(&resident, r#"return { watch = { colors = "colors" } }"#).unwrap();
            let town = Town::with_log(dir.join("past.log"));
            let mut words = town.words.subscribe();
            wire(&Lua::new(), &town, &resident, 0).unwrap();
            let bg = |s: &str| Body::of(s);
            assert_eq!(until(&mut words, |_| true).await.body, bg("bg=#000"));
            std::fs::remove_file(&file).unwrap();
            assert_eq!(until(&mut words, |_| true).await.body, Body::none());
            assert!(!town.present.borrow().contains_key("colors"), "a gone file is not present");
            std::fs::write(&file, "bg=#fff").unwrap();
            assert_eq!(until(&mut words, |_| true).await.body, bg("bg=#fff"));
            // an editor's save that takes the file away a moment: whether the gap is seen is the
            // filesystem's to say, but what the file holds after it always is
            std::fs::remove_file(&file).unwrap();
            std::fs::write(&file, "bg=#abc").unwrap();
            until(&mut words, |w| w.body == bg("bg=#abc")).await;
            end(&town);
            std::fs::remove_dir_all(&dir).ok();
        });
    }

    // a relative watch path is beside the resident, wherever the town was started
    #[test]
    fn a_watch_path_is_beside_its_resident() {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        tokio::task::LocalSet::new().block_on(&rt, async {
            let dir = std::env::temp_dir().join(format!("town-beside-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("colors"), "bg=#000").unwrap();
            let resident = dir.join("r.lua");
            std::fs::write(&resident, r#"return { watch = { colors = "colors" } }"#).unwrap();
            let town = Town::with_log(dir.join("past.log"));
            let mut words = town.words.subscribe();
            wire(&Lua::new(), &town, &resident, 0).unwrap();
            let said = until(&mut words, |_| true).await;
            assert_eq!((said.kind.as_str(), &said.body), ("colors", &Body::of("bg=#000")));
            end(&town);
            std::fs::remove_dir_all(&dir).ok();
        });
    }
}
