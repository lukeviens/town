//! The square: how other runtimes reach town — talk words in, or listen
//! for the news. `reach` is the same two moves from a shell.

use std::rc::Rc;

use tokio::io::AsyncWriteExt;
use tokio::net::unix::OwnedWriteHalf;
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::broadcast::{error::RecvError, Receiver};

use crate::paths;
use crate::town::{Lines, Town};
use crate::word::Word;

pub async fn run(listener: UnixListener, town: Rc<Town>) {
    loop {
        if let Ok((conn, _)) = listener.accept().await {
            tokio::task::spawn_local(at_square(conn, town.clone()));
        }
    }
}

// The first line says how the connection speaks:
//   listen  — hear the news only (one-way, out)
//   join    — hear the news AND talk words in, on the one connection (two-way)
//   else    — a speaker: every line is a word (one-way, in)
// `join` is what a long-lived program wants: one socket, no process per word.
async fn at_square(conn: UnixStream, town: Rc<Town>) {
    let (r, w) = conn.into_split();
    let mut lines = Lines::new(r);
    let Some(first) = lines.next().await else { return };
    match first.trim() {
        "listen" => news(w, &town, town.words.subscribe()).await,
        "join" => {
            // listening before hearing: what their first word makes town say, they hear
            let words = town.words.subscribe();
            let talker = town.clone();
            let heard = async move {
                while let Some(line) = lines.next().await {
                    said(&talker, &line);
                }
            };
            // hear words in and stream news out at once; when they hang up, both end.
            tokio::select! {
                _ = heard => {}
                _ = news(w, &town, words) => {}
            }
        }
        _ => {
            said(&town, &first);
            while let Some(line) = lines.next().await {
                said(&town, &line);
            }
        }
    }
}

// a line from someone at the square: said in town if it is a word, and not one only town itself
// makes
fn said(town: &Town, line: &str) {
    match town.hear("square", line) {
        Some(word) if town.admits(&word) => town.talk(word),
        Some(word) => {
            town.error("square", Some(&word.kind), "refused: only town makes those".into())
        }
        None => {}
    }
}

// Stream the present, then every word thereafter, to a listener. Ends when they hang up.
async fn news(mut w: OwnedWriteHalf, town: &Town, mut words: Receiver<Word>) {
    for word in town.present_words() {
        if w.write_all(format!("{}\n", word.to_line()).as_bytes()).await.is_err() {
            return;
        }
    }
    loop {
        match words.recv().await {
            Ok(word) => {
                if w.write_all(format!("{}\n", word.to_line()).as_bytes()).await.is_err() {
                    return;
                }
            }
            Err(RecvError::Lagged(_)) => continue,
            Err(_) => return,
        }
    }
}

/// `town talk` / `town listen` from a shell — reach the square from outside.
/// ask: say a word and wait for its answer — the past word of the same kind carrying the same
/// `id` — then print the answer's body. For a program that needs a reply, not only to be heard.
/// The word is given an `id` if it has none. False if no one answered before `wait` ran out.
pub fn ask(word: &str, wait: std::time::Duration) -> bool {
    use std::io::{BufRead, BufReader as SyncReader, ErrorKind, Write};
    use std::os::unix::net::UnixStream as SyncStream;
    // a non-word would be said as an `error` no one waits for: refuse it here, and say why
    if let Err(e) = serde_json::from_str::<Word>(word) {
        eprintln!("town: ask: not a word: {e}");
        return false;
    }
    let Ok(mut word) = serde_json::from_str::<serde_json::Value>(word) else { return false };
    let Ok(mut s) = SyncStream::connect(paths::square()) else {
        eprintln!("town: no one at the square (is town up?)");
        return false;
    };
    if !word["body"].is_object() {
        word["body"] = serde_json::json!({});
    }
    let id = match word["body"]["id"].as_str() {
        Some(id) => id.to_string(),
        None => {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default();
            format!("{}-{}", std::process::id(), now.as_millis())
        }
    };
    word["body"]["id"] = serde_json::json!(id);
    let kind = word["kind"].clone();
    // joined first, so the answer cannot arrive before we are there to hear it
    if writeln!(s, "join").and_then(|_| writeln!(s, "{word}")).is_err() {
        return false;
    }
    let until = std::time::Instant::now() + wait;
    s.set_read_timeout(Some(std::time::Duration::from_millis(500))).ok();
    let mut lines = SyncReader::new(s).lines();
    while std::time::Instant::now() < until {
        match lines.next() {
            Some(Ok(line)) => {
                let Ok(w) = serde_json::from_str::<serde_json::Value>(&line) else { continue };
                if w["kind"] == kind && w["tense"] == "past" && w["body"]["id"] == id.as_str() {
                    println!("{}", w["body"]);
                    return true;
                }
                // refused at the square: no answer is coming
                if w["kind"] == "error"
                    && w["body"]["from"] == "square"
                    && w["body"]["heard"] == kind
                {
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

pub fn reach(verb: &str, word: Option<&String>) {
    use std::io::{BufRead, BufReader as SyncReader, Write};
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
                let _ = writeln!(s, "{word}");
            }
        }
        "listen" => {
            let _ = writeln!(s, "listen");
            let mut out = std::io::stdout();
            for word in SyncReader::new(s).lines().map_while(Result::ok) {
                // whoever was reading has gone (a pipe closed): stop quietly, not with a panic
                if writeln!(out, "{word}").and_then(|_| out.flush()).is_err() {
                    break;
                }
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // what the square hears and will not say is said as an `error`: a line that is not a word, and
    // a word only town makes
    #[test]
    fn the_square_says_what_it_will_not_say() {
        let dir = std::env::temp_dir().join(format!("town-square-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let town = Town::with_log(dir.join("past.log"));
        let mut words = town.words.subscribe();
        said(&town, r#"{"body":{"x":1}}"#);
        let w = words.try_recv().expect("a non-word is an error word");
        assert_eq!((w.kind.as_str(), w.body["from"].as_str()), ("error", Some("square")));
        assert!(w.body["message"].as_str().unwrap().starts_with("not a word"));
        said(&town, r#"{"kind":"work","tense":"future","body":{"cmd":"true"}}"#);
        let w = words.try_recv().expect("a refusal is an error word");
        assert_eq!(w.body["heard"], "work");
        said(&town, "");
        assert!(words.try_recv().is_err(), "a blank line is nothing");
        std::fs::remove_dir_all(&dir).ok();
    }
}
