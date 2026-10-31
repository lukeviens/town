//! The square: how other runtimes reach town — talk words in, or listen
//! for the news. `reach` is the same two moves from a shell.

use std::rc::Rc;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::unix::OwnedWriteHalf;
use tokio::net::{UnixListener, UnixStream};

use crate::paths;
use crate::town::Town;
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
// `join` is what a long-lived runtime (HS) wants: one socket, no process per word.
async fn at_square(conn: UnixStream, town: Rc<Town>) {
    let (r, w) = conn.into_split();
    let mut lines = BufReader::new(r).lines();
    let first = match lines.next_line().await {
        Ok(Some(l)) => l,
        _ => return,
    };
    match first.trim() {
        "listen" => news(w, &town).await,
        "join" => {
            let talker = town.clone();
            let heard = async move {
                while let Ok(Some(line)) = lines.next_line().await {
                    if let Some(word) = Word::from_line(&line) {
                        talker.talk(word);
                    }
                }
            };
            // hear words in and stream news out at once; when they hang up, both end.
            tokio::select! {
                _ = heard => {}
                _ = news(w, &town) => {}
            }
        }
        _ => {
            if let Some(word) = Word::from_line(&first) {
                town.talk(word);
            }
            while let Ok(Some(line)) = lines.next_line().await {
                if let Some(word) = Word::from_line(&line) {
                    town.talk(word);
                }
            }
        }
    }
}

// Stream the present, then every word thereafter, to a listener. Ends when they hang up.
async fn news(mut w: OwnedWriteHalf, town: &Town) {
    let mut words = town.words.subscribe();
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
            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
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
    let Ok(mut word) = serde_json::from_str::<serde_json::Value>(word) else {
        eprintln!("town: ask: not a word");
        return false;
    };
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
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
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
            }
            Some(Err(e)) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => continue,
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
