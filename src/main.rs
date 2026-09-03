//! town — a small VM that composes your desktop out of words. Residents (residents/*.lua)
//! listen for words and talk words back; the present is their fold, the past keeps them
//! all. Other runtimes reach the same town through the square.
//!
//!   town              open the town (run its programs)
//!   town talk <json>  say a word
//!   town listen       hear the words

// the engine lives in the library crate (src/lib.rs); the binary just orchestrates it.
use town::town::Town;
use town::{paths, residents, square};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("talk") => return square::reach("talk", Some(&word_line(&args[2..]))),
        Some("listen") => return square::reach("listen", None),
        None => {} // no verb → open the town below
        Some(other) => {
            // never boot on a stray arg — that would remove the square and spawn a rogue server
            eprintln!("town: unknown command '{other}' (expected: talk, listen, or nothing to open)");
            return;
        }
    }

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let local = tokio::task::LocalSet::new();
    local.block_on(&rt, async move {
        let path = paths::square();
        // don't steal a live square — if one already answers, we're already up
        if std::os::unix::net::UnixStream::connect(&path).is_ok() {
            eprintln!("town: already up");
            return;
        }
        std::fs::remove_file(&path).ok(); // no one home; clear any stale socket
        let listener = match tokio::net::UnixListener::bind(&path) {
            Ok(l) => l,
            Err(_) => {
                eprintln!("town: the square is taken — town is already up");
                return;
            }
        };
        let town = Town::new();
        town.compact(128 * 1024); // keep the past log bounded; only recent facts feed past()
        let lua = unsafe { mlua::Lua::unsafe_new() }; // trusted programs; full stdlib
        if let Err(e) = residents::install(&lua, &town) {
            eprintln!("town: {e}");
            return;
        }
        if let Err(e) = residents::open(&lua, &town) {
            eprintln!("town: {e}");
            return;
        }
        square::run(listener, town).await; // residents run alongside on the same runtime
    });
}

// A word to talk, from the command line: raw JSON, or `[TENSE] KIND key=value …`.
// TENSE is an optional leading `past`/`present`/`future`; without it, a present fact.
fn word_line(args: &[String]) -> String {
    use town::word::{Tense, Word};
    if let Some(a) = args.first() {
        if a.starts_with('{') {
            return a.clone();
        }
    }
    let (tense, rest) = match args.first().map(String::as_str) {
        Some("past") => (Tense::Past, &args[1..]),
        Some("future") => (Tense::Future, &args[1..]),
        Some("present") => (Tense::Present, &args[1..]),
        _ => (Tense::Present, args),
    };
    match rest.first() {
        Some(kind) => {
            let mut body = serde_json::Map::new();
            for kv in &rest[1..] {
                if let Some((k, v)) = kv.split_once('=') {
                    body.insert(k.to_string(), serde_json::Value::String(v.to_string()));
                }
            }
            Word { kind: kind.clone(), tense, body: serde_json::Value::Object(body) }.to_line()
        }
        None => String::new(),
    }
}
