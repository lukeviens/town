//! town — a VM whose one instruction is the word, { kind, tense, body, when }, said on the square.
//! Its programs are residents, the files at the paths it is opened with: they listen there and
//! talk back, and any other program may talk and listen there too. A fact, a present-tense word,
//! folds into the present (the last of each kind) and is kept in the log; past and future words
//! pass through.
//!
//!   town <path>…      open a town: programs, or folders of them, in order
//!   town talk <word>  say a word
//!   town listen       hear the words
//!   town ask <word>   say a word, and wait for its answer

// town itself is the library (src/lib.rs); this is how it is started, and reached from a shell
use town::town::Town;
use town::{paths, residents, square};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("talk") => return square::reach("talk", Some(said(&args[2..])), None),
        Some("listen") => return square::reach("listen", None, args.get(2)),
        // ask: say a word, wait for its answer, print it. TOWN_WAIT seconds (default 1800), else
        // exit 1.
        Some("ask") => {
            let wait = std::env::var("TOWN_WAIT").ok().and_then(|s| s.parse().ok()).unwrap_or(1800);
            let answered = square::ask(said(&args[2..]), std::time::Duration::from_secs(wait));
            std::process::exit(if answered { 0 } else { 1 });
        }
        None => {
            // a town is its programs: with none named, there is nothing to open
            eprintln!(
                "usage: town <path>…        open a town: programs, or folders of them, in order
       town talk <word>     say a word
       town listen [language] hear the words
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
    // whole, but a link left as it is: it is followed each time the town opens, so a moved one is
    // seen
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
        town.compact(); // keep the past log bounded; only recent facts feed past()
        town::work::answer(&town); // future `work`, `after` and `join`, answered off the runtime
        let lua = unsafe { mlua::Lua::unsafe_new() }; // trusted programs; full stdlib
        if let Err(e) = town::lua::install(&lua, &town) {
            eprintln!("town: {e}");
            std::process::exit(1);
        }
        tokio::task::spawn_local(residents::ending(town.clone())); // programs stop with the town
        residents::keep(&lua, &town, programs); // opens the town, and again on each change
        square::run(listener, town).await; // residents run alongside on the same runtime
    });
}

// A word to talk, from the command line: JSON as it is, or `[TENSE] KIND key=value …`, said in
// `words`, town's own language for a word on a line, so town reads it as it reads any language.
// TENSE is an optional leading `past`/`present`/`future`; without it, a present fact.
fn said(args: &[String]) -> square::Said {
    if let Some(json) = args.first().filter(|a| a.starts_with('{')) {
        return square::Said::Json(json.clone());
    }
    // Tense is the word's, never the body's. Written here it is silently ignored: the word goes out
    // as a present fact, folds into the present forever, and carries the tense it meant as data.
    // (`kind` can be a real body field; `tense` never is.)
    if args.iter().any(|a| a.starts_with("tense=")) {
        eprintln!("town: tense belongs to the word, not its body.");
        eprintln!("town talk [past|present|future] <kind> key=value …");
        std::process::exit(2);
    }
    // what follows the kind is its body, key=value each: anything else is no word
    let tensed = ["past", "present", "future"].contains(&args.first().map_or("", String::as_str));
    if args.len() == tensed as usize {
        eprintln!("town: a word has a kind");
        eprintln!("town talk [past|present|future] <kind> key=value …");
        std::process::exit(2);
    }
    if let Some(bad) =
        args.iter().skip(1 + tensed as usize).find(|a| !a.contains('=') || a.starts_with('='))
    {
        eprintln!("town: `{bad}` is not key=value");
        eprintln!("town talk [past|present|future] <kind> key=value …");
        std::process::exit(2);
    }
    // a tab or a line's end would end a word in `words`, or a part of one: such a word goes as JSON
    if args.iter().any(|a| a.contains(['\t', '\n'])) {
        eprintln!("town: a tab or a line's end cannot be said short; say the word in JSON");
        std::process::exit(2);
    }
    square::Said::Words(args.to_vec())
}
