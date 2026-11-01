//! town — a VM whose one instruction is the word, { kind, tense, body, when }, said on the square.
//! Its programs are residents, Lua files at the paths it is opened with: they listen there and
//! talk back, and any other program may talk and listen there too. A fact, a present-tense word,
//! folds into the present (the last of each kind) and is kept in the log; past and future words
//! pass through.
//!
//!   town <path>…      open a town: Lua files, or folders of them, in order
//!   town talk <word>  say a word
//!   town listen       hear the words
//!   town ask <word>   say a word, and wait for its answer

// the engine lives in the library crate (src/lib.rs); the binary just orchestrates it.
use town::town::Town;
use town::{paths, residents, square};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("talk") => return square::reach("talk", Some(&word_line(&args[2..]))),
        Some("listen") => return square::reach("listen", None),
        // ask: say a word, wait for its answer, print it. TOWN_WAIT seconds (default 1800), else
        // exit 1.
        Some("ask") => {
            let wait = std::env::var("TOWN_WAIT").ok().and_then(|s| s.parse().ok()).unwrap_or(1800);
            let answered =
                square::ask(&word_line(&args[2..]), std::time::Duration::from_secs(wait));
            std::process::exit(if answered { 0 } else { 1 });
        }
        None => {
            // a town is its programs: with none named, there is nothing to open
            eprintln!(
                "usage: town <path>…        open a town: Lua files, or folders of them, in order
       town talk <word>     say a word
       town listen          hear the words
       town ask <word>      say a word, and wait for its answer"
            );
            std::process::exit(2);
        }
        Some(_) => {}
    }
    // never boot on a path that is not there — a misspelled verb included — rather than open half
    // a town
    let programs: Vec<std::path::PathBuf> =
        args[1..].iter().map(std::path::PathBuf::from).collect();
    if let Some(missing) = programs.iter().find(|p| !p.exists()) {
        eprintln!("town: no such program: {}", missing.display());
        std::process::exit(2);
    }
    // whole, but not resolved: a link is followed each time the town opens, so a moved one is seen
    let programs: Vec<std::path::PathBuf> =
        programs.iter().map(|p| std::path::absolute(p).unwrap_or(p.clone())).collect();

    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
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
            Err(e) => {
                // no one answered, so it is not taken: say why it would not open
                // a failure, so launchd tries again; only `already up` is a clean exit
                eprintln!("town: cannot open the square at {}: {e}", path.display());
                std::process::exit(1);
            }
        };
        let town = Town::new();
        town.compact(128 * 1024); // keep the past log bounded; only recent facts feed past()
        town::work::obey(&town); // future `work` is run outside, off the runtime
        let lua = unsafe { mlua::Lua::unsafe_new() }; // trusted programs; full stdlib
        if let Err(e) = residents::install(&lua, &town) {
            eprintln!("town: {e}");
            std::process::exit(1);
        }
        residents::open(&lua, &town, &programs);
        residents::keep(&lua, &town, programs);
        square::run(listener, town).await; // residents run alongside on the same runtime
    });
}

// A word to talk, from the command line: raw JSON, or `[TENSE] KIND key=value …`.
// TENSE is an optional leading `past`/`present`/`future`; without it, a present fact.
fn word_line(args: &[String]) -> String {
    use town::word::{Tense, Word};
    if let Some(json) = args.first().filter(|a| a.starts_with('{')) {
        return json.clone();
    }
    let (tense, rest) = match args.first().map(String::as_str) {
        Some("past") => (Tense::Past, &args[1..]),
        Some("future") => (Tense::Future, &args[1..]),
        Some("present") => (Tense::Present, &args[1..]),
        _ => (Tense::Present, args),
    };
    let Some((kind, pairs)) = rest.split_first() else { return String::new() };
    let mut body = serde_json::Map::new();
    for (k, v) in pairs.iter().filter_map(|kv| kv.split_once('=')) {
        // Tense is the word's, never the body's. Written here it is silently ignored: the word
        // goes out as a present fact, folds into the present forever, and carries the tense it
        // meant as data. (`kind` can be a real body field; `tense` never is.)
        if k == "tense" {
            eprintln!("town: tense belongs to the word, not its body.");
            eprintln!("town talk [past|present|future] <kind> key=value …");
            std::process::exit(2);
        }
        body.insert(k.into(), v.into());
    }
    Word::new(kind, tense, body.into()).to_line()
}
