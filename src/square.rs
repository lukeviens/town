//! The square: how other programs reach town — talk words in, or listen
//! for the news. `reach` is the same two moves from a shell.

use std::rc::Rc;

use tokio::net::unix::OwnedWriteHalf;
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::broadcast::{error::RecvError, Receiver};

use crate::body::Body;
use crate::connection::{say, word, Heard, Speech};
use crate::language::Language;
use crate::parts::Parts;
use crate::paths;
use crate::town::Town;
use crate::word::{Tense, Word};

pub async fn run(listener: UnixListener, town: Rc<Town>) {
    loop {
        if let Ok((conn, _)) = listener.accept().await {
            tokio::task::spawn_local(at_square(conn, town.clone()));
        }
    }
}

// The first line says how the connection speaks, and in what language, JSON if it does not say:
//   listen [out]       — hear the news only (one-way, out)
//   join [in] [out]    — hear the news AND talk words in, on the one connection (two-way); out is
//                        in, if it is not said
//   talk [in]          — talk words in (one-way, in)
//   else               — a word in JSON, and every line after it
// `join` is what a long-lived program wants: one socket, no process per word.
async fn at_square(conn: UnixStream, town: Rc<Town>) {
    let (r, mut w) = conn.into_split();
    let mut heard = Heard::new(r);
    let Some(first) = heard.next().await else { return };
    let mut said_as = first.split_whitespace();
    let verb = said_as.next().unwrap_or_default();
    if !["listen", "join", "talk"].contains(&verb) {
        // the first line was a word too
        let json = Language::json();
        if let Some(first) = word(&town, "square", &first) {
            said(&town, first);
        }
        return talk(&town, Speech::new(heard, json)).await;
    }
    let languages: Result<Vec<_>, _> = said_as.map(|name| town.languages.get(name)).collect();
    let languages = match languages {
        Ok(f) if f.len() > 2 => {
            Err("a connection names its language, or the one it says and the one it hears".into())
        }
        named => named,
    };
    let languages = match languages {
        Ok(f) => f,
        Err(e) => {
            // told to whoever asked, in the one language it surely hears, and to the town
            let told = Parts::map([("from", "square"), ("message", e.as_str())]);
            let told = Word::new("error", Tense::Past, Body::of(told));
            say(&mut w, &told, &Language::Json, &town.languages).await.ok();
            return town.error("square", None, e);
        }
    };
    let json = Language::json();
    let (into, out) = match languages.as_slice() {
        [] => (json.clone(), json),
        [one] => (one.clone(), one.clone()),
        [into, out, ..] => (into.clone(), out.clone()),
    };
    match verb {
        "listen" => news(w, &town, town.words.subscribe(), &into).await,
        "talk" => talk(&town, Speech::new(heard, into)).await,
        _ => {
            // listening before hearing: what their first word makes town say, they hear
            let words = town.words.subscribe();
            // hear words in and stream news out at once; when they hang up, both end.
            tokio::select! {
                _ = talk(&town, Speech::new(heard, into)) => {}
                _ = news(w, &town, words, &out) => {}
            }
        }
    }
}

// what someone at the square says, said in town, until they stop
async fn talk(town: &Town, mut speech: Speech<tokio::net::unix::OwnedReadHalf>) {
    while let Some(w) = speech.next(town, "square").await {
        said(town, w);
    }
}

// a word from someone at the square: said in town, if it is not one only town itself makes
fn said(town: &Town, word: Word) {
    if town.admits(&word) {
        town.talk(word);
    } else {
        town.error("square", Some(&word.kind), "refused: only town makes those".into());
    }
}

// Stream the present, then every word thereafter, to a listener, in its language: a word the
// language cannot hold is passed over. Ends when they hang up.
async fn news(mut w: OwnedWriteHalf, town: &Town, mut words: Receiver<Word>, language: &Language) {
    for word in town.present_words() {
        if say(&mut w, &word, language, &town.languages).await.is_err() {
            return;
        }
    }
    loop {
        match words.recv().await {
            Ok(word) => {
                if say(&mut w, &word, language, &town.languages).await.is_err() {
                    return;
                }
            }
            Err(RecvError::Lagged(_)) => continue,
            Err(_) => return,
        }
    }
}

/// A word from a shell: JSON, or the words of `words`, a tab between each when said.
pub enum Said {
    Json(String),
    Words(Vec<String>),
}

impl Said {
    /// the line that says the word
    fn line(&self) -> String {
        match self {
            Said::Json(line) => line.clone(),
            Said::Words(words) => words.join("\t"),
        }
    }

    /// the lines that say it at the square, as `verb` says it
    fn lines(&self, verb: &str) -> String {
        match self {
            Said::Json(line) if verb == "talk" => line.clone(),
            Said::Json(line) => format!("{verb}\n{line}"),
            Said::Words(_) => format!("{verb} words json\n{}", self.line()),
        }
    }
}

/// ask: say a word and wait for its answer — the past word of the same kind carrying the same
/// `id` — then print the answer's body. For a program that needs a reply, not only to be heard.
/// The word is given an `id` if it has none. False if no one answered before `wait` ran out.
pub fn ask(word: Said, wait: std::time::Duration) -> bool {
    use std::io::{BufRead, BufReader as SyncReader, ErrorKind, Write};
    use std::os::unix::net::UnixStream as SyncStream;
    let now =
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    let fresh = format!("{}-{}", std::process::id(), now.as_millis());
    let (word, kind, id) = match word {
        Said::Json(line) => {
            // a non-word would be said as an `error` no one waits for: refuse it here, and say why
            if let Err(e) = crate::connection::read(&line) {
                eprintln!("town: ask: not a word: {e}");
                return false;
            }
            let Ok(mut word) = serde_json::from_str::<serde_json::Value>(&line) else {
                return false;
            };
            if !word["body"].is_object() {
                word["body"] = serde_json::json!({});
            }
            let id = word["body"]["id"].as_str().map_or(fresh, String::from);
            word["body"]["id"] = serde_json::json!(id);
            let kind = word["kind"].as_str().unwrap_or_default().to_string();
            (Said::Json(word.to_string()), kind, id)
        }
        Said::Words(mut words) => {
            let tense = ["past", "present", "future"].contains(&words.first().map_or("", |w| w));
            let kind = words.get(tense as usize).cloned().unwrap_or_default();
            let id = match words.iter().find_map(|w| w.strip_prefix("id=")) {
                Some(id) => id.to_string(),
                None => {
                    words.push(format!("id={fresh}"));
                    fresh
                }
            };
            (Said::Words(words), kind, id)
        }
    };
    let Ok(mut s) = SyncStream::connect(paths::square()) else {
        eprintln!("town: no one at the square (is town up?)");
        return false;
    };
    // joined first, so the answer cannot arrive before we are there to hear it
    let said = word.line();
    if writeln!(s, "{}", word.lines("join")).is_err() {
        return false;
    }
    let until = std::time::Instant::now() + wait;
    s.set_read_timeout(Some(std::time::Duration::from_millis(500))).ok();
    let mut lines = SyncReader::new(s).lines();
    while std::time::Instant::now() < until {
        match lines.next() {
            Some(Ok(line)) => {
                let Ok(w) = serde_json::from_str::<serde_json::Value>(&line) else { continue };
                if w["kind"] == kind.as_str()
                    && w["tense"] == "past"
                    && w["body"]["id"] == id.as_str()
                {
                    println!("{}", w["body"]);
                    return true;
                }
                // refused at the square, or not heard as a word: no answer is coming
                let error = w["kind"] == "error" && w["body"]["from"] == "square";
                if error && (w["body"]["heard"] == kind.as_str() || w["body"]["said"] == said) {
                    eprintln!("town: ask: {}", w["body"]["message"].as_str().unwrap_or("refused"));
                    return false;
                }
            }
            Some(Err(e)) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
                continue
            }
            _ => return false, // the square closed
        }
    }
    false
}

/// `town talk` / `town listen` from a shell — reach the square from outside. A listener hears in
/// the language it names, JSON if it names none.
pub fn reach(verb: &str, word: Option<Said>, language: Option<&String>) {
    use std::io::Write;
    use std::os::unix::net::UnixStream as SyncStream;
    // `listen` is persistent, so it waits for the square to come up; `talk` is
    // fire-and-forget, so it fails fast rather than piling up hung processes.
    let tries = if verb == "listen" { 30 } else { 1 };
    let mut s = None;
    for i in 0..tries {
        match SyncStream::connect(paths::square()) {
            Ok(conn) => {
                s = Some(conn);
                break;
            }
            Err(_) if i + 1 < tries => std::thread::sleep(std::time::Duration::from_millis(100)),
            Err(_) => {}
        }
    }
    let mut s = match s {
        Some(s) => s,
        None => {
            eprintln!("town: no one at the square (is town up?)");
            return;
        }
    };
    match verb {
        "talk" => {
            if let Some(word) = word {
                let _ = writeln!(s, "{}", word.lines("talk"));
            }
        }
        "listen" => {
            let _ = writeln!(s, "listen {}", language.map_or("", String::as_str));
            // the bytes as they are said, a binary language too; whoever was reading may have gone
            // (a pipe closed), and that ends it quietly
            let _ = std::io::copy(&mut s, &mut std::io::stdout());
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::body::Body;

    // a word only town makes, said at the square, is refused, and the refusal is an `error`
    #[test]
    fn the_square_says_what_it_will_not_say() {
        let dir = std::env::temp_dir().join(format!("town-square-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let town = Town::with_log(dir.join("past.log"));
        let mut words = town.words.subscribe();
        said(
            &town,
            Word::new(
                "work",
                crate::word::Tense::Future,
                Body::of(serde_json::json!({"cmd": "true"})),
            ),
        );
        let w = words.try_recv().expect("a refusal is an error word");
        assert_eq!((w.kind.as_str(), w.body.value()["from"].as_str()), ("error", Some("square")));
        assert_eq!(w.body.value()["heard"], "work");
        said(&town, Word::new("note", crate::word::Tense::Past, Body::none()));
        assert_eq!(words.try_recv().unwrap().kind, "note", "anyone may say the rest");
        std::fs::remove_dir_all(&dir).ok();
    }

    // a connection that names a language town does not speak is told so, in JSON, and let go
    #[test]
    fn a_connection_is_told_it_names_no_language() {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        tokio::task::LocalSet::new().block_on(&rt, async {
            let dir = std::env::temp_dir().join(format!("town-unspoken-{}", std::process::id()));
            let town = Town::with_log(dir.join("past.log"));
            for asked in ["listen cbro\n", "join json json json\n"] {
                let (mut ours, theirs) = UnixStream::pair().unwrap();
                tokio::task::spawn_local(at_square(theirs, town.clone()));
                ours.write_all(asked.as_bytes()).await.unwrap();
                let mut told = String::new();
                tokio::io::BufReader::new(ours).read_line(&mut told).await.unwrap();
                let told = crate::connection::read(told.trim_end()).unwrap();
                assert_eq!(told.kind, "error", "{asked}");
                assert!(told.body.value()["message"].as_str().unwrap().contains("language"));
            }
        });
    }
}
