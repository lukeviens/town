//! lua: town's own resident that speaks for each Lua file in the paths town opens with. A Lua file
//! cannot run itself, so this one runs it, and says its words for it. Its README is beside it.
//!
//! - **finds**: the `.lua` files of `town/residents` that cannot run themselves, in town's order.
//! - **speaks for**: each, said at `town/residents/<its path>`, so under this one's place; each of
//!   its words names it, `by`.
//! - **hears**: what its files hear, which town tells it as they come, and passes each word to each
//!   file that hears it. What a file reads, a kind's present or its past, lua reads from town
//!   (`town/read`): town is the one that knows, and lua keeps no copy.
//! - **again**: a file saved goes and comes again alone, as town says a save settled (a past
//!   `town/open`). A library, which the files after it are written with, and a file not opened,
//!   one new or one that failed, open them all again, in a new Lua.
//! - **a word**: a table. A part is nil, a string, a number, a boolean, or a table: a list if its
//!   keys are 1 to n, else parts by name. A nothing in a list is `null`, so the list keeps its
//!   places.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::mpsc;
use std::time::Duration;

use mlua::{Function, Lua, Table, Value};
use town::body::Body;
use town::language::{Languages, DEEP};
use town::parts::Parts;
use town::word::{hears, is_kind, place, Tense, Word};

/// How a resident is written, read into each file before it.
const RESIDENT: &str = include_str!("resident");

/// Where the paths are, and, under it, each resident here.
const RESIDENTS: &str = "town/residents";

/// Where the mark a list carries is kept.
const LIST: &str = "town.list";

/// How long a file waits for town to answer a read, before it is the file's error.
const READ: Duration = Duration::from_secs(5);

fn main() {
    // what it is: what tells it its files, and what asks it to open one again; town tells it what
    // its files hear, as each comes
    println!(r#"{{"kinds":["/{RESIDENTS}","town/open"]}}"#);
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                return;
            }
        }
    });
    let me = Me::default();
    *me.here.heard.borrow_mut() = Some(rx);
    loop {
        // what came as a file read from town, first, in the order it came
        let aside = me.here.aside.borrow_mut().pop_front();
        if let Some((w, read)) = aside {
            me.heard(w, read);
            continue;
        }
        let line = me.here.heard.borrow().as_ref().map(|rx| rx.recv());
        // town hung up: nothing more to hear, nor to say to
        let Some(Ok(line)) = line else { return };
        if let Ok(w) = town::voice::read(&line) {
            me.heard(w, None);
        }
    }
}

/// What every Lua it makes looks through: the languages, and where it says and hears its words.
/// Each Lua's windows hold it, so it is shared.
#[derive(Default)]
struct Here {
    languages: Languages,
    /// what town tells it, a line at a time; and what came while a file waited for a read, set
    /// aside to be heard next, in order
    heard: RefCell<Option<mpsc::Receiver<String>>>,
    aside: RefCell<std::collections::VecDeque<(Word, Option<u64>)>>,
    /// the file, by when it came, whose present is being read as it comes: a fact set aside
    /// meanwhile is in what it reads, so it is not told it again
    reading: std::cell::Cell<Option<u64>>,
    reads: std::cell::Cell<u64>,
}

impl Here {
    /// What town keeps of `kind` in `tense`: its fact now, or its facts before, oldest first; for
    /// a pattern, each kind's, by kind. Asked of town, and waited for: what else comes meanwhile is
    /// set aside, to be heard once this read is answered.
    fn read(&self, kind: &str, tense: &str) -> mlua::Result<Parts> {
        self.reads.set(self.reads.get() + 1);
        let id = format!("lua/{}", self.reads.get());
        let ask = Parts::map([("kind", kind), ("tense", tense), ("id", id.as_str())]);
        self.say(&Word::new("town/read", Tense::Future, Body::of(ask)), None);
        let heard = self.heard.borrow();
        let rx = heard.as_ref().ok_or_else(|| mlua::Error::runtime("town is not here to read"))?;
        loop {
            let line = rx.recv_timeout(READ).map_err(|_| {
                mlua::Error::runtime(format!("town did not answer a read of {kind}"))
            })?;
            let Ok(w) = town::voice::read(&line) else { continue };
            let ours = w.kind == "town/read" && w.tense == Tense::Past;
            if ours && w.body.part("id").and_then(Parts::text) == Some(id.as_str()) {
                return Ok(w.body.part("body").cloned().unwrap_or_default());
            }
            let reading = self.reading.get().filter(|_| w.tense == Tense::Present);
            self.aside.borrow_mut().push_back((w, reading));
        }
    }

    /// A word, said: by the file named `by`, if one says it, or by lua itself.
    fn say(&self, w: &Word, by: Option<&str>) {
        let mut line = town::voice::line(w);
        if let Some(by) = by {
            line.pop();
            line.push_str(&format!(r#","by":{}}}"#, serde_json::Value::from(by)));
        }
        let mut out = std::io::stdout().lock();
        let _ = writeln!(out, "{line}").and_then(|()| out.flush());
    }
}

/// A file it speaks for: its name, which is its whole path but the first `/`, its kinds, its
/// `listen`, and its place to run in.
struct File {
    path: PathBuf,
    name: String,
    /// when it came, in the order files come: one that came hearing a word is not told it too
    came: u64,
    kinds: Vec<String>,
    listen: Option<Function>,
}

/// What a file came as: a resident, by its name; a library; or nothing, as it failed.
#[derive(Clone, PartialEq)]
enum Came {
    Resident,
    Library,
    Failed,
}

/// lua itself: what it shares with its Lua, the paths, the Lua it runs its files in, each file it
/// read and what it came as, and the residents among them, each with when it came.
#[derive(Default)]
struct Me {
    here: Rc<Here>,
    paths: RefCell<Vec<PathBuf>>,
    lua: RefCell<Option<Lua>>,
    ran: RefCell<Vec<(PathBuf, Came)>>,
    files: RefCell<Vec<File>>,
    /// how many files have come; and how many had, as this batch of them began to
    came: std::cell::Cell<u64>,
    batch: std::cell::Cell<u64>,
}

impl Me {
    /// What failed: a `town/error` from `from`, by the file of that name if it came, hearing
    /// `heard` if a word was in hand. One of a file, town tells back, with its `place`, as it tells
    /// every error of one lua speaks for (`heard`); one of lua's own it does not, so lua tells it
    /// to each of its files that hears it.
    fn failed(&self, from: &str, by: Option<&str>, heard: Option<&str>, message: String) {
        let mut body = Parts::map([("from", from), ("message", message.as_str())]);
        if let Some(heard) = heard {
            body.set("heard", heard);
        }
        let error = Word::new("town/error", Tense::Past, Body::of(body));
        self.here.say(&error, by);
        if by.is_none() {
            self.tell(&error, None, self.came.get(), None);
        }
    }

    /// A word heard: the paths open every file again; a save is asked; a resident's fact tells the
    /// languages it is; and each file that hears it is told it. Not `now`: within it, each hears
    /// the present first.
    fn heard(&self, w: Word, read: Option<u64>) {
        if w.is_now() {
            return;
        }
        // those that come as it is heard were greeted with the present, which holds it: not told it
        let before = self.came.get();
        // the languages a resident is, from its fact, so a file may read and write them
        if w.tense == Tense::Present && w.kind.starts_with(&format!("{RESIDENTS}/")) {
            self.here.languages.are(&w.kind, &spoken(&w.body));
        }
        // an error of one of its files is told to the rest, not to it: by its place, under lua's
        let mut of = (w.kind == "town/error").then(|| self.of(&w.body)).flatten();
        // a file whose fact town refused is dropped, and the refusal told to the rest
        if let Some(refused) = self.refused(&w) {
            self.drop_file(&refused);
            of = Some(refused);
        }
        if w.kind == RESIDENTS && w.tense == Tense::Present {
            let paths = w.body.parts().list().into_iter().flatten();
            *self.paths.borrow_mut() = paths.filter_map(Parts::text).map(PathBuf::from).collect();
            self.open();
        } else if w.kind == "town/open" && w.tense == Tense::Past {
            // a save, settled by town: of the files it touched, those lua speaks for open again
            let paths = w.body.part("paths").and_then(Parts::list).into_iter().flatten();
            let saved: Vec<PathBuf> = paths.filter_map(Parts::text).map(PathBuf::from).collect();
            self.saved(&saved);
        }
        // what it read is only what lua heard before it came: the rest, town tells it now
        let read = read.filter(|_| self.heard_before(&w.kind));
        self.tell(&w, of.as_deref(), before, read);
    }

    /// The name of the file an error is of, by its `place`, which ends in it.
    fn of(&self, body: &Body) -> Option<String> {
        let place = body.part("place").and_then(Parts::text)?;
        let files = self.files.borrow();
        let of = files.iter().find(|f| place.ends_with(&format!("/{}", f.name)));
        of.map(|f| f.name.clone())
    }

    /// The name of a file whose fact town refused, if this error says so: what it heard is the
    /// fact lua said for it.
    fn refused(&self, w: &Word) -> Option<String> {
        if w.kind != "town/error" {
            return None;
        }
        let heard = w.body.part("heard").and_then(Parts::text)?;
        let name = heard.strip_prefix(&format!("{RESIDENTS}/"))?;
        self.files.borrow().iter().any(|f| f.name == name).then(|| name.to_string())
    }

    /// A file whose fact town refused did not come: lua speaks for it no more, and opens it again
    /// as it would one that failed.
    fn drop_file(&self, name: &str) {
        let gone: Vec<File> = self.files.borrow_mut().extract_if(.., |f| f.name == name).collect();
        let mut ran = self.ran.borrow_mut();
        for file in &gone {
            self.here.languages.are(&mine(&file.name), &[]);
            if let Some(came) = ran.iter_mut().find(|(p, _)| *p == file.path) {
                came.1 = Came::Failed;
            }
        }
    }

    /// Each file that hears `w` is told it, in the order they came, but the one it is `of`, those
    /// that came after the `before`th, and the one that `read` it as it came.
    fn tell(&self, w: &Word, of: Option<&str>, before: u64, read: Option<u64>) {
        let hearing: Vec<(String, Function)> = self
            .files
            .borrow()
            .iter()
            .filter(|f| f.came <= before && Some(f.name.as_str()) != of && Some(f.came) != read)
            .filter(|f| f.kinds.iter().any(|k| hears(k, &w.kind)))
            .filter_map(|f| f.listen.clone().map(|l| (f.name.clone(), l)))
            .collect();
        let Some(lua) = self.lua.borrow().clone() else { return };
        for (name, listen) in hearing {
            self.listen(&lua, &name, &listen, w);
        }
    }

    /// A file listens to a word, and whatever it answers is said as its: a word, or nil for none. A
    /// Lua error, or an answer that is no word, is a `town/error` from it, but one it made hearing
    /// a `town/error`, which is not said: two that hear them would make each other's without end.
    fn listen(&self, lua: &Lua, name: &str, listen: &Function, w: &Word) {
        let said = to_lua(lua, &town::voice::parts(w)).and_then(|t| listen.call::<Value>(t));
        let said = said.map(|said| (!said.is_nil()).then(|| to_word(said)));
        let failed = match said {
            Ok(None) => return,
            Ok(Some(Ok(word))) => return self.here.say(&word, Some(name)),
            Ok(Some(Err(e))) => format!("answered with a non-word: {e}"),
            Err(e) => e.to_string(),
        };
        if w.kind != "town/error" {
            self.failed(named(name), Some(name), Some(&w.kind), failed);
        }
    }

    /// What one save touched, as town says it once the save settles: of those files, the ones it
    /// read, or its walk finds now, open again.
    fn saved(&self, paths: &[PathBuf]) {
        let found = lua_files(&self.paths.borrow());
        let ran = self.ran.borrow().clone();
        let mine = |p: &PathBuf| found.contains(p) || ran.iter().any(|(f, _)| f == p);
        let asked: Vec<PathBuf> = paths.iter().filter(|p| mine(p)).cloned().collect();
        if !asked.is_empty() {
            self.again(&asked);
        }
    }

    /// Every file it speaks for goes, and each of the paths' Lua files comes, in a new Lua, so a
    /// resident keeps nothing but what the present holds.
    fn open(&self) {
        for file in self.files.take() {
            self.leaves(&file);
        }
        // its files are trusted, as town's own are: all of Lua's own, `debug` and C modules too
        let lua = unsafe { Lua::unsafe_new() };
        if let Err(e) = install(&lua, &self.here) {
            self.failed("lua", None, None, format!("cannot make Lua: {e}"));
        }
        *self.lua.borrow_mut() = Some(lua.clone());
        let files = lua_files(&self.paths.borrow());
        let ran = self.comes(&lua, &files);
        *self.ran.borrow_mut() = ran;
        self.said();
    }

    /// What one save asked: each file it speaks for goes and comes again, alone; one deleted only
    /// goes. A library, or a file not opened, opens them all again: the files after it are written
    /// with it.
    fn again(&self, asked: &[PathBuf]) {
        let Some(lua) = self.lua.borrow().clone() else { return self.open() };
        let ran = self.ran.borrow().clone();
        let came = |path: &PathBuf| ran.iter().find(|(f, _)| f == path).map(|(_, c)| c.clone());
        if asked.iter().any(|p| came(p) != Some(Came::Resident)) {
            return self.open();
        }
        let going: Vec<File> = {
            let mut files = self.files.borrow_mut();
            files.extract_if(.., |f| asked.contains(&f.path)).collect()
        };
        for file in &going {
            self.leaves(file);
        }
        // one deleted only goes, as does one that runs now: town runs it, as a program
        let coming: Vec<PathBuf> =
            asked.iter().filter(|p| p.exists() && !town::program::runs(p)).cloned().collect();
        let came = self.comes(&lua, &coming);
        let library = came.iter().any(|(_, c)| *c == Came::Library);
        let mut ran = self.ran.borrow_mut();
        ran.retain(|(f, _)| !asked.contains(f));
        ran.extend(came);
        drop(ran);
        // one that is a library now: the files after it are written with it
        if library {
            return self.open();
        }
        self.said();
    }

    /// All it speaks for is said: what those that went owned, held while they came again, is let
    /// go now, of those that did not.
    fn said(&self) {
        self.here.say(&Word::new(RESIDENTS, Tense::Past, Body::none()), None);
    }

    /// A file it spoke for goes: said so, at its place; and the languages it was, no longer spoken.
    fn leaves(&self, file: &File) {
        let kind = format!("{RESIDENTS}/{}", file.name);
        self.here.say(&Word::new(kind, Tense::Present, Body::none()), None);
        self.here.languages.are(&mine(&file.name), &[]);
    }

    /// Whether lua heard `kind` before the file now coming: its own kinds, or another file's.
    /// One of this batch is not counted: town tells lua what it newly hears as each comes, and lua
    /// passes that on to all of the batch, the file now coming among them.
    fn heard_before(&self, kind: &str) -> bool {
        let own = [format!("/{RESIDENTS}"), "town/open".into()];
        let files = self.files.borrow();
        let before = files.iter().filter(|f| f.came <= self.batch.get());
        own.iter().chain(before.flat_map(|f| f.kinds.iter())).any(|k| hears(k, kind))
    }

    /// Each of `files` comes, in order. What failed is said last, once each listens, so one may
    /// hear it. Each file, and what it came as.
    fn comes(&self, lua: &Lua, files: &[PathBuf]) -> Vec<(PathBuf, Came)> {
        self.batch.set(self.came.get());
        let mut failed = Vec::new();
        let mut ran = Vec::new();
        for path in files {
            match self.wire(lua, path) {
                Ok(came) => ran.push((path.clone(), came)),
                Err(e) => {
                    failed.push((named(&name(path)).to_string(), e.to_string()));
                    ran.push((path.clone(), Came::Failed));
                }
            }
        }
        for (from, message) in failed {
            self.failed(&from, None, None, message);
        }
        ran
    }

    /// A file, read in its own place (`place`). A resident is said at `town/residents/<its name>`,
    /// so under lua's place; then what it talked as it was read; then the present of its kinds.
    /// One that fails has done nothing.
    fn wire(&self, lua: &Lua, path: &Path) -> mlua::Result<Came> {
        let src = std::fs::read_to_string(path)?;
        let name = name(path);
        let held: Held = Rc::new(RefCell::new(Some(Vec::new())));
        let env = self.place(lua, path, &name, &held)?;
        let it: Option<Table> =
            lua.load(src.as_str()).set_name(named(&name)).set_environment(env).eval()?;
        let Some(it) = it else {
            // a library: words for the files after it, each of which reads it into its own place.
            // What it talked as it was read is said, by lua, as it is no resident to say it
            libraries(lua)?.push(lua.create_sequence_from([named(&name).to_string(), src])?)?;
            for w in held.take().unwrap_or_default() {
                self.here.say(&w, None);
            }
            return Ok(Came::Library);
        };
        // nothing else: a misspelled field would do nothing, and say nothing of why
        for key in it.pairs::<Value, Value>().filter_map(Result::ok).map(|(k, _)| k) {
            let key = key.to_string().unwrap_or_default();
            if !["kinds", "listen", "own", "languages"].contains(&key.as_str()) {
                return Err(mlua::Error::runtime(format!(
                    "a resident has no `{key}`: it is `kinds`, `listen`, `own` and `languages`"
                )));
            }
        }
        let strings = |t: Table| t.sequence_values::<String>().filter_map(Result::ok).collect();
        let own: Vec<String> = it.get::<Option<Table>>("own")?.map(strings).unwrap_or_default();
        let listen: Option<Function> = it.get("listen")?;
        let heard: Vec<String> = it.get::<Option<Table>>("kinds")?.map(strings).unwrap_or_default();
        let heard = if listen.is_some() { heard } else { Vec::new() };
        let mut languages = Vec::new();
        if let Some(spoken) = it.get::<Option<Table>>("languages")? {
            for pair in spoken.pairs::<String, Value>() {
                let (name, grammar) = pair?;
                languages.push((name, to_parts(grammar)?));
            }
        }
        // what it is, said whole, so one that cannot be holds nothing, and town hears it
        let mut body = Parts::Map(Vec::new());
        body.set("kinds", Parts::List(heard.iter().map(|k| k.as_str().into()).collect()));
        body.set("own", Parts::List(own.iter().map(|k| k.as_str().into()).collect()));
        let spoken_now = languages.clone();
        if !languages.is_empty() {
            body.set("languages", Parts::Map(languages));
        }
        let kind = format!("{RESIDENTS}/{name}");
        self.here.say(&Word::new(kind, Tense::Present, Body::of(body)), None);
        // its languages, spoken at once, so a file after it may read and write them as it is read
        // but not one another is: town refuses it, and that one's stays
        let free = |(n, _): &(String, Parts)| self.here.languages.get(n).is_err();
        let spoken_now: Vec<(String, Parts)> = spoken_now.into_iter().filter(free).collect();
        self.here.languages.are(&mine(&name), &spoken_now);
        for w in held.take().unwrap_or_default() {
            self.here.say(&w, Some(&name));
        }
        self.came.set(self.came.get() + 1);
        let came = self.came.get();
        let file =
            File { path: path.to_path_buf(), name: name.clone(), came, kinds: heard, listen };
        // its present, before any word, in town's order: read from town, of the kinds lua heard
        // before it came; of those lua hears now it did not, town tells lua as it said the fact
        if let Some(listen) = &file.listen {
            let mut present = BTreeMap::new();
            self.here.reading.set(Some(came));
            for k in &file.kinds {
                // a read town does not answer: it has come, so it goes, holding nothing
                let read = self.here.read(k, "present").inspect_err(|_| {
                    self.here.reading.set(None);
                    self.leaves(&file)
                })?;
                match (is_kind(k), read) {
                    (_, Parts::None) => {}
                    (true, body) => drop(present.insert(k.clone(), body)),
                    (false, Parts::Map(all)) => present.extend(all),
                    (false, _) => {}
                }
            }
            self.here.reading.set(None);
            for (kind, body) in present.into_iter().filter(|(kind, _)| self.heard_before(kind)) {
                let fact = Word::new(kind, Tense::Present, Body::of(body));
                self.listen(lua, &name, listen, &fact);
            }
        }
        self.files.borrow_mut().push(file);
        Ok(Came::Resident)
    }

    /// Where a file runs, `_ENV`, all its own: its `talk`, which speaks as it, however late; its
    /// `present` and `past`, read from town; how a resident is written; and each library read
    /// before it the first time it came, so one that comes again alone has those it had. Nothing is
    /// shared through Lua: one that comes again begins again.
    fn place(&self, lua: &Lua, path: &Path, name: &str, held: &Held) -> mlua::Result<Table> {
        let (here, by, still) = (self.here.clone(), name.to_string(), held.clone());
        // a library read into it says nothing as it is read: it said its words once, read alone
        let quiet = Rc::new(std::cell::Cell::new(false));
        let hush = quiet.clone();
        let talk = lua.create_function(move |_, w: Value| {
            let w =
                to_word(w).map_err(|e| mlua::Error::runtime(format!("talked a non-word: {e}")))?;
            if hush.get() {
                return Ok(());
            }
            match still.borrow_mut().as_mut() {
                Some(held) => held.push(w),
                None => here.say(&w, Some(&by)),
            }
            Ok(())
        })?;
        // a kind's fact now, read from town, the one that knows
        let here = self.here.clone();
        let present = lua.create_function(move |lua, kind: String| {
            // what names no kind, `a//b`, is nothing, and asks none
            let inner = place(&kind);
            if !is_kind(&kind) && (inner == kind || !(inner.is_empty() || is_kind(inner))) {
                return Ok(Value::Nil);
            }
            to_lua(lua, &here.read(&kind, "present")?)
        })?;
        // its facts before, oldest first, read from town: kept by none but town
        let here = self.here.clone();
        let past =
            lua.create_function(move |lua, kind: String| to_lua(lua, &here.read(&kind, "past")?))?;
        let env = lua.create_table_from([("talk", talk), ("present", present), ("past", past)])?;
        let all = lua.create_table_from([("__index", lua.globals())])?;
        env.set_metatable(Some(all));
        lua.load(RESIDENT).set_name("resident").set_environment(env.clone()).exec()?;
        let before = before(lua, path)?;
        quiet.set(true);
        let read =
            libraries(lua)?.sequence_values::<Table>().take(before).try_for_each(|library| {
                let library = library?;
                let (name, src): (String, String) = (library.get(1)?, library.get(2)?);
                lua.load(src).set_name(name).set_environment(env.clone()).exec()
            });
        quiet.set(false);
        read.map(|()| env)
    }
}

/// What a file talks while it is read, held until it has come, and said then, in order: one that
/// fails has done nothing. Nothing held, once it has come.
type Held = Rc<RefCell<Option<Vec<Word>>>>;

/// A file's name: its whole path, but the first `/`.
fn name(path: &Path) -> String {
    path.to_string_lossy().trim_start_matches('/').to_string()
}

/// What a file is called: the last of its name, as its fact says, `hi.lua`.
fn named(name: &str) -> &str {
    name.rsplit('/').next().unwrap_or(name)
}

/// Where the languages a file is are kept in lua's own, by its name: spoken as it comes, before
/// town says its fact back.
fn mine(name: &str) -> String {
    format!("lua/{name}")
}

/// The Lua files of `paths`, as town walks them: those named `.lua` that cannot run themselves,
/// as a program can.
fn lua_files(paths: &[PathBuf]) -> Vec<PathBuf> {
    let lua = |f: &Path| f.extension().is_some_and(|x| x == "lua") && f.is_file();
    town::residents::files(paths, |f| lua(f) && !town::program::runs(f))
}

/// The languages a resident's fact says it is, each a grammar by its name.
fn spoken(body: &Body) -> Vec<(String, Parts)> {
    match body.part("languages") {
        Some(Parts::Map(all)) => all.clone(),
        _ => Vec::new(),
    }
}

/// What every file's place reads through: how town reads a kind, `hears`; a nothing in a list,
/// `null`, and `list{}`; and the languages, `parts` and `bytes`.
fn install(lua: &Lua, here: &Rc<Here>) -> mlua::Result<()> {
    let rule = lua.create_function(|_, (k, s): (String, String)| Ok(hears(&k, &s)))?;
    lua.globals().set("hears", rule)?;
    lua.globals().set("null", Value::NULL)?;
    let mark = lua.create_table()?;
    mark.raw_set("__name", LIST)?;
    lua.set_named_registry_value(LIST, mark)?;
    lua.globals().set(
        "list",
        lua.create_function(|lua, t: Option<Table>| {
            let t = match t {
                Some(t) => t,
                None => lua.create_table()?,
            };
            t.set_metatable(Some(lua.named_registry_value::<Table>(LIST)?));
            Ok(t)
        })?,
    )?;
    // town itself, to name in a join: `join(kind, { TOWN, … })`
    lua.globals().set("TOWN", std::env::var("TOWN").ok())?;
    // town's watch, to join by its path: `join(kind, { WATCH, path })`
    lua.globals().set("WATCH", std::env::var("WATCH").ok())?;
    let reads = here.clone();
    lua.globals().set(
        "parts",
        lua.create_function(move |lua, (name, bytes): (String, mlua::String)| {
            let parts = reads.languages.parts(&name, &bytes.as_bytes());
            to_lua(lua, &parts.map_err(mlua::Error::runtime)?)
        })?,
    )?;
    let writes = here.clone();
    lua.globals().set(
        "bytes",
        lua.create_function(move |lua, (name, parts): (String, Value)| {
            let parts =
                to_parts(parts).map_err(|e| mlua::Error::runtime(format!("{name}: {e}")))?;
            let bytes = writes.languages.bytes(&name, &parts);
            lua.create_string(bytes.map_err(mlua::Error::runtime)?)
        })?,
    )?;
    lua.set_named_registry_value(LIBRARIES, lua.create_table()?)?;
    lua.set_named_registry_value(BEFORE, lua.create_table()?)?;
    Ok(())
}

/// Where the libraries a Lua has read are kept, in order: each a name and its source.
const LIBRARIES: &str = "town.libraries";

/// How many libraries each file came after, the first time it came, by its path.
const BEFORE: &str = "town.before";

fn libraries(lua: &Lua) -> mlua::Result<Table> {
    lua.named_registry_value(LIBRARIES)
}

/// How many libraries `path` came after, the first time it came.
fn before(lua: &Lua, path: &Path) -> mlua::Result<usize> {
    let before: Table = lua.named_registry_value(BEFORE)?;
    let at = path.to_string_lossy().into_owned();
    if let Some(n) = before.get::<Option<usize>>(at.as_str())? {
        return Ok(n);
    }
    let n = libraries(lua)?.raw_len();
    before.set(at, n)?;
    Ok(n)
}

/// parts, as Lua holds them
fn to_lua(lua: &Lua, p: &Parts) -> mlua::Result<Value> {
    Ok(match p {
        Parts::None => Value::Nil,
        Parts::Text(t) => Value::String(lua.create_string(t)?),
        Parts::Bytes(b) => Value::String(lua.create_string(b)?),
        Parts::Number(n) => match (n.parse::<i64>(), n.parse::<f64>()) {
            (Ok(i), _) => Value::Integer(i),
            (_, Ok(f)) => Value::Number(f),
            _ => Value::String(lua.create_string(n)?),
        },
        Parts::Bool(b) => Value::Boolean(*b),
        // a Lua list holds no nil, so a nothing in it is `null`; and it is marked, so even an empty
        // one is a list again
        Parts::List(l) => {
            let t = lua.create_table_with_capacity(l.len(), 0)?;
            if let Ok(mark) = lua.named_registry_value::<Table>(LIST) {
                t.set_metatable(Some(mark));
            }
            for p in l {
                t.raw_push(match p {
                    Parts::None => Value::NULL,
                    p => to_lua(lua, p)?,
                })?;
            }
            Value::Table(t)
        }
        Parts::Map(m) => {
            let t = lua.create_table_with_capacity(0, m.len())?;
            for (n, p) in m {
                t.raw_set(n.as_str(), to_lua(lua, p)?)?;
            }
            Value::Table(t)
        }
    })
}

/// what Lua holds, as parts
fn to_parts(v: Value) -> mlua::Result<Parts> {
    read(v, 0)
}

fn read(v: Value, deep: usize) -> mlua::Result<Parts> {
    // a table that holds itself goes on without end
    if deep > DEEP {
        return Err(mlua::Error::runtime("parts too deep: a table that holds itself?"));
    }
    Ok(match v {
        Value::Nil => Parts::None,
        Value::LightUserData(u) if u.0.is_null() => Parts::None,
        Value::Boolean(b) => b.into(),
        Value::Integer(i) => i.into(),
        Value::Number(n) => n.into(),
        // a string that is not text is bytes
        Value::String(s) => match s.to_str() {
            Ok(t) => t.to_string().into(),
            Err(_) => Parts::Bytes(s.as_bytes().to_vec()),
        },
        Value::Table(t) => {
            let pairs: Vec<(Value, Value)> = t.pairs().collect::<mlua::Result<_>>()?;
            let n = t.raw_len();
            let marked = t.metatable().is_some_and(|m| {
                m.raw_get::<Option<String>>("__name").ok().flatten().as_deref() == Some(LIST)
            });
            if marked || (n > 0 && pairs.len() == n) {
                let l = t.sequence_values().map(|v| read(v?, deep + 1));
                Parts::List(l.collect::<mlua::Result<_>>()?)
            } else {
                let mut m = Parts::Map(Vec::new());
                for (k, v) in pairs {
                    let name = match k {
                        Value::String(s) => s.to_string_lossy(),
                        Value::Integer(i) => i.to_string(),
                        Value::Number(n) => n.to_string(),
                        k => return Err(not_a_part(&k, "name")),
                    };
                    m.set(name, read(v, deep + 1)?);
                }
                m
            }
        }
        v => return Err(not_a_part(&v, "part")),
    })
}

fn not_a_part(v: &Value, what: &str) -> mlua::Error {
    mlua::Error::runtime(format!("a {} is no {what}", v.type_name()))
}

/// a table, as a word: its parts, as any resident's words are read, at the root. A file speaks for
/// none: its words name no `by`, so it may not say one is there, a fact under `town/residents/`.
fn to_word(v: Value) -> mlua::Result<Word> {
    let w = town::voice::word_at("", &to_parts(v)?).map_err(mlua::Error::runtime)?;
    if w.tense == Tense::Present && w.kind.starts_with(&format!("{RESIDENTS}/")) {
        return Err(mlua::Error::runtime("a file of Lua speaks for none: its words name no `by`"));
    }
    Ok(w)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// lua, a Lua, and a file's place in it, to read code in
    fn placed() -> (Me, Lua, Table) {
        let me = Me::default();
        let lua = Lua::new();
        install(&lua, &me.here).unwrap();
        let held: Held = Default::default();
        let env = me.place(&lua, Path::new("/t.lua"), "t.lua", &held).unwrap();
        (me, lua, env)
    }

    /// what code read in a file's place comes to
    fn read<T: mlua::FromLuaMulti>(lua: &Lua, env: &Table, code: &str) -> T {
        lua.load(code).set_environment(env.clone()).eval().unwrap()
    }

    // an empty table is parts by name, and `list{}` an empty list; a list that comes to Lua is
    // marked as one, and goes back as one; and a nothing in it is null, so it keeps its places
    #[test]
    fn a_list_is_a_list() {
        let (_me, lua, env) = placed();
        let said = |code: &str| to_parts(read(&lua, &env, code)).unwrap();
        assert_eq!(said("return {}"), Parts::Map(vec![]));
        assert_eq!(said("return list{}"), Parts::List(vec![]));
        assert_eq!(said("return list{ 1, 2 }"), Parts::List(vec![1i64.into(), 2i64.into()]));
        for list in [Parts::List(vec![]), Parts::List(vec![1i64.into(), Parts::None, "x".into()])] {
            let held = to_lua(&lua, &list).unwrap();
            assert_eq!(to_parts(held).unwrap(), list);
        }
    }

    // a resident of clauses hears the kinds they name, those it names, and every kind if a clause
    // names none; errors only if a clause names them. A word wakes the first clause that hears its
    // kind, or none.
    #[test]
    fn a_resident_hears_what_its_clauses_and_its_kinds_name() {
        let (_me, lua, env) = placed();
        let kinds = |clauses: &str| -> Vec<String> {
            read(&lua, &env, &format!("return react{{ {clauses} }}.kinds"))
        };
        assert_eq!(kinds(r#"on("/peer", print), on("note", print)"#), ["/peer", "note"]);
        assert_eq!(kinds(r#"kinds = { "theme" }, on("note", print)"#), ["note", "theme"]);
        assert_eq!(kinds(r#"on("a", print), on({ tense = "past" }, print)"#), ["/"], "but errors");
        assert_eq!(kinds(r#"on("town/error", print), on({}, print)"#), ["/", "town/error"]);
        let code =
            r#"return react{ on("/peer", function(w) return w.kind end), on("note", print) }"#;
        let listen: Function = read::<Table>(&lua, &env, code).get("listen").unwrap();
        let heard = |kind: &str| -> Option<String> {
            let w: Table = lua.load(format!("return {{ kind = {kind:?} }}")).eval().unwrap();
            listen.call(w).unwrap()
        };
        assert_eq!((heard("peer"), heard("peer/a")), (Some("peer".into()), Some("peer/a".into())));
        assert_eq!((heard("peers"), heard("note/a")), (None, None), "beside it, or under `note`");
    }

    // a file's own `hears` is its own: `on` keeps town's rule
    #[test]
    fn a_files_own_hears_is_not_the_rule() {
        let (_me, lua, env) = placed();
        read::<()>(&lua, &env, "function hears() return true end");
        let matched: bool = read(&lua, &env, r#"return on("a", print).match({ kind = "b" })"#);
        assert!(!matched);
    }

    // what names no kind is nothing, and is asked of none: town is not even here
    #[test]
    fn what_names_no_kind_is_nothing() {
        let (_me, lua, env) = placed();
        let read = |code: &str| -> String { read(&lua, &env, code) };
        assert_eq!(read(r#"return tostring(present("")) .. tostring(present("a//b"))"#), "nilnil");
    }

    // a file that cannot be posted is its resident's error, not lost without a word
    #[test]
    fn a_failed_post_is_an_error() {
        let (_me, lua, env) = placed();
        let dir = std::env::temp_dir().join(format!("lua-post-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("folder")).unwrap();
        let post = |path: &Path| {
            let code = format!("post({:?}, 'x')", path.to_str().unwrap());
            lua.load(code).set_environment(env.clone()).exec()
        };
        assert!(post(Path::new("/nonexistent/dir/f")).is_err(), "no folder to write in");
        assert!(post(&dir.join("folder")).is_err(), "a folder is no file");
        assert!(!dir.join("folder.writing").exists(), "what was written aside is gone");
        post(&dir.join("f")).unwrap();
        assert_eq!(std::fs::read_to_string(dir.join("f")).unwrap(), "x");
        std::fs::remove_dir_all(&dir).ok();
    }
}
