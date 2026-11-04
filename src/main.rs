//! town: a VM whose one instruction is the word, `{ kind, tense, body, when }`, said on the square.
//!
//! - **a town**: its residents, the files at the paths it is opened with; they, and any other
//!   program, talk and listen there.
//! - **a fact**: a present word, folded into the present, the last of each kind, and logged; past
//!   and future words pass through.
//! - **watching**: town watches nothing; `reload` asks it to open a resident again as it is saved.

// the binary: town started, and town reached from a shell; town is the library
use town::body::Body;
use town::parts::Parts;
use town::town::Town;
use town::word::{Tense, Word};
use town::{paths, program, residents, square, voice};

/// How town is called; said when it is called with nothing.
const USAGE: &str = "\
town <path>…                            open a town: residents, or folders of them, in order
town talk <word>                        say a word, and go
town listen [language] [for pattern…]   hear the words: the present, then each as it is said
town ask <word>                         say a word, and wait for its answer";

fn main() {
    let args: Vec<String> = std::env::args().collect();
    // `TOWN`: town itself, for its programs to call, `$TOWN talk`; one that names no file is
    // replaced. And town's own programs, `town-watch` among them, beside it: first on its world's
    // PATH, so it finds those it was built with, wherever it is
    if let Ok(me) = std::env::current_exe() {
        if !std::env::var_os("TOWN").is_some_and(|t| std::path::Path::new(&t).is_file()) {
            std::env::set_var("TOWN", &me);
        }
        if let Some(beside) = me.parent() {
            let path = std::env::var_os("PATH").unwrap_or_default();
            let paths = [beside.to_path_buf()].into_iter().chain(std::env::split_paths(&path));
            if let Ok(path) = std::env::join_paths(paths) {
                std::env::set_var("PATH", path);
            }
        }
    }
    match args.get(1).map(String::as_str) {
        // talk: say a word and go; exit 1 if no town is at the square to hear it
        Some("talk") => std::process::exit(if square::talk(&word(&args[2..])) { 0 } else { 1 }),
        // listen: exit 1 if no town is at the square, 2 for arguments it cannot read
        Some("listen") => std::process::exit(square::listen(&args[2..])),
        // ask: wait TOWN_WAIT seconds (1800 if unset) for the answer, else exit 1; exit 2 for a
        // word that cannot be asked
        Some("ask") => {
            let wait = std::env::var("TOWN_WAIT").ok().and_then(|s| s.parse().ok()).unwrap_or(1800);
            std::process::exit(square::ask(word(&args[2..]), std::time::Duration::from_secs(wait)));
        }
        None => {
            // a town is its residents: with none named, there is nothing to open
            eprintln!("{USAGE}");
            std::process::exit(2);
        }
        Some(_) => {}
    }
    // a missing path, a misspelled verb among them, opens no town rather than half of one; each
    // is made whole as the town keeps it (`residents::keep`)
    let paths: Vec<std::path::PathBuf> = args[1..].iter().map(std::path::PathBuf::from).collect();
    if let Some(missing) = paths.iter().find(|p| !p.exists()) {
        eprintln!("town: no such resident: {}", missing.display());
        std::process::exit(2);
    }

    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let local = tokio::task::LocalSet::new();
    local.block_on(&rt, async move {
        let path = paths::square();
        // a square that answers is a town already up: it is not taken from it
        if std::os::unix::net::UnixStream::connect(&path).is_ok() {
            eprintln!("town: already up");
            return;
        }
        std::fs::remove_file(&path).ok(); // none answers: a socket left there is let go
        let square = match tokio::net::UnixListener::bind(&path) {
            Ok(l) => l,
            Err(e) => {
                // a failure, so launchd tries again; only `already up` ends well
                eprintln!("town: cannot open the square at {}: {e}", path.display());
                std::process::exit(1);
            }
        };
        let town = Town::with_log(paths::past());
        // the log, bounded: the present, and the recent past `past` reads
        town.compact();
        // programs stop with the town
        tokio::task::spawn_local(program::ending(town.clone()));
        // opens the town, and each resident again as it is saved; the whole town in a new Lua each
        // time: its residents are trusted, so all of Lua's own
        residents::keep(|| unsafe { mlua::Lua::unsafe_new() }, &town, paths);
        square::run(square, town).await; // residents run alongside on the same runtime
    });
}

/// `said`, or an exit saying why not and how a word is said
fn word(args: &[String]) -> Word {
    said(args).unwrap_or_else(|e| {
        eprintln!("town: {e}");
        eprintln!("a word is a line of JSON, or [past|present|future] <kind> key=value …");
        std::process::exit(2);
    })
}

/// A word from the command line: a line of JSON, or said short, `[tense] kind key=value …`,
/// present if no tense is said, its body the pairs as text, or none. Else why it is no word.
fn said(args: &[String]) -> Result<Word, String> {
    if let Some(json) = args.first().filter(|a| a.starts_with('{')) {
        return voice::read(json).map_err(|e| format!("not a word: {e}"));
    }
    // tense is the word's: as a pair it would make a fact holding the tense it meant, so it is
    // refused
    if args.iter().any(|a| a.starts_with("tense=")) {
        return Err("tense belongs to the word, not its body".into());
    }
    let tense = args.first().and_then(|t| Tense::named(t));
    let Some((kind, pairs)) = args[tense.is_some() as usize..].split_first() else {
        return Err("a word has a kind".into());
    };
    let mut body = Parts::map::<&str, Parts>([]);
    for pair in pairs {
        match pair.split_once('=') {
            Some((key, value)) if !key.is_empty() => body.set(key, value),
            _ => return Err(format!("`{pair}` is not key=value")),
        }
    }
    let body = if pairs.is_empty() { Body::none() } else { Body::of(body) };
    Ok(Word::new(kind.as_str(), tense.unwrap_or(Tense::Present), body))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn said(args: &[&str]) -> Result<Word, String> {
        super::said(&args.iter().map(|a| a.to_string()).collect::<Vec<_>>())
    }

    // a word said short: its tense, else present; its kind; its pairs as text, `=` and tabs and
    // all; or no body
    #[test]
    fn a_word_is_said_short() {
        let w = said(&["past", "shout", "text=hi\tthere", "n=", "eq=a=b"]).unwrap();
        assert_eq!((w.kind.as_str(), w.tense), ("shout", Tense::Past));
        let body = serde_json::json!({ "text": "hi\tthere", "n": "", "eq": "a=b" });
        assert_eq!(w.body, Body::of(body));
        let w = said(&["pastry"]).unwrap();
        assert_eq!((w.kind.as_str(), w.tense, w.body.is_none()), ("pastry", Tense::Present, true));
        assert_eq!(said(&["future", "a=b"]).unwrap().kind, "a=b", "a kind is any text");
    }

    // or in JSON, as town reads any line; and what is no word is refused, and why
    #[test]
    fn a_word_is_said_in_json_or_refused() {
        let w = said(&[r#"{"kind":"q","tense":"future","body":{"a":1}}"#]).unwrap();
        assert_eq!((w.kind.as_str(), w.tense), ("q", Tense::Future));
        for (args, why) in [
            (&[r#"{"kind":"q","x":1}"#][..], "not a word"),
            (&[], "a word has a kind"),
            (&["past"], "a word has a kind"),
            (&["note", "text"], "`text` is not key=value"),
            (&["note", "=x"], "`=x` is not key=value"),
            (&["note", "tense=past"], "tense belongs to the word"),
        ] {
            let e = said(args).unwrap_err();
            assert!(e.contains(why), "{args:?}: {e}");
        }
    }
}
