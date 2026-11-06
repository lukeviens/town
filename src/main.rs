//! town: a VM whose one instruction is the word, `{ kind, tense, body, when, by }`, said at town.
//!
//! - **a town**: its residents, the files at the paths it is opened with; they, and any other
//!   program, talk and listen there.
//! - **a fact**: a present word, folded into the present, the last of each kind, and logged; past
//!   and future words pass through.

// the binary: town started, and town reached from a shell; town is the library
use town::town::{System, Town};
use town::{paths, program, residents, square, unix};

use std::path::PathBuf;

/// How town is called; said when it is called with nothing.
const USAGE: &str = r#"town <path>…          open a town: residents, or folders of them, in order
town '<first word>'   be at the square as it says: stdin said there, all told it on stdout
                      {}  to talk; {"kinds":["/"]}  to hear; {"name":"luke"}  to come by a name"#;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        // a first word, which no path is: at the square as it says, and piped there
        Some(first) if first.starts_with('{') && args.len() == 2 => {
            std::process::exit(square::pipe(first))
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
    let paths: Vec<PathBuf> = args[1..].iter().map(PathBuf::from).collect();
    if let Some(missing) = paths.iter().find(|p| !p.exists()) {
        eprintln!("town: no such resident: {}", missing.display());
        std::process::exit(2);
    }

    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let local = tokio::task::LocalSet::new();
    local.block_on(&rt, async move {
        // one town to a home: one keeps its lock while it runs, and another finds it kept
        let held = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(paths::lock());
        let held = held.ok().filter(unix::lock);
        let Some(_held) = held else {
            eprintln!("town: already up");
            return;
        };
        // its log read before its square is there, so none comes to a town that cannot open
        let town = match Town::opened(paths::past(), std::env::vars_os().collect(), System) {
            Ok(town) => town,
            Err(e) => {
                eprintln!("town: cannot open its log, {}: {e}", paths::past().display());
                std::process::exit(1);
            }
        };
        // bound beside its place, then put there whole, so one who finds it can reach it
        let path = paths::square();
        let new = path.with_extension("new");
        std::fs::remove_file(&new).ok();
        let square = tokio::net::UnixListener::bind(&new).and_then(|l| {
            std::fs::rename(&new, &path)?;
            Ok(l)
        });
        let square = match square {
            Ok(l) => l,
            Err(e) => {
                // a failure, so its keeper tries again; only `already up` ends well
                eprintln!("town: cannot open the square at {}: {e}", path.display());
                std::process::exit(1);
            }
        };
        // programs stop with the town
        tokio::task::spawn_local(program::ending(town.clone()));
        // opens the town, and each resident again as it is saved
        residents::keep(&town, paths);
        square::run(square, town).await; // residents run alongside on the same runtime
    });
}
