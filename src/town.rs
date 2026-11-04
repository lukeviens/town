//! The town: the present, and the words said.
//!
//! - **`talk`**: folds a fact into the present, keeps it in the log, and sends every word on to all
//!   who listen.
//! - **`said`**: the one door, through which a resident's word comes, if it may say it.
//! - **who is here**: the residents' facts under `town/residents`, from which ownership and the
//!   languages are read.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::ffi::OsString;
use std::path::PathBuf;
use std::rc::{Rc, Weak};

use tokio::sync::mpsc;

use crate::body::Body;
use crate::log;
use crate::parts::Parts;
use crate::voice::read;
use crate::word::{hears, is_kind, place, Tense, Word, ERROR, KIND};

/// A program joined, or a socket connected: who it is, its kind, owned, and its run, among the
/// town's tasks. Taken out of `joins`, its run ends and its kind is let go.
pub struct Joined {
    pub who: Resident,
    pub(crate) _here: Here,
    pub(crate) _run: crate::program::Ends,
}

/// A resident: who says a word, a file of Lua, a program, a join, one at the square, or town
/// itself, number 0. While it is here, this is its fact, at its `place`.
///
/// - **what it says it is**: `kinds`, what it hears; `own`, what only it says while it is here;
///   `speaks`, its language, JSON if it names none, or `[says, hears]`; `languages`, those it is,
///   each a grammar by its name. Nothing else: a misspelled field would hear nothing, and say
///   nothing of why.
/// - **what town gives it**: its number, its name, its file; `at`, the kind it is joined at, else
///   the root; `square`, whether it came there, so it may not say town's own; `by`, the one that
///   speaks for it, if one does.
#[derive(Clone, Default, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct Resident {
    #[serde(skip_deserializing)]
    pub id: u64,
    #[serde(skip_deserializing)]
    pub name: String,
    #[serde(default)]
    pub kinds: Vec<String>,
    #[serde(default)]
    pub own: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speaks: Option<crate::voice::Speaks>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub languages: BTreeMap<String, Parts>,
    #[serde(skip_deserializing, skip_serializing_if = "Option::is_none")]
    pub at: Option<String>,
    #[serde(skip_deserializing, skip_serializing_if = "std::ops::Not::not")]
    pub square: bool,
    #[serde(skip)]
    pub file: Option<PathBuf>,
    /// its place, if the resident that speaks for it said it is there: `<its place>/<name>`
    #[serde(skip)]
    pub placed: Option<String>,
    /// the number of the resident that speaks for it, if one does
    #[serde(skip_deserializing, skip_serializing_if = "Option::is_none")]
    pub by: Option<u64>,
    /// a file of Lua: it speaks for none, as its words have no `by`
    #[serde(skip)]
    pub lua: bool,
}

/// Where town's residents are: the fact, the paths town is opened with; under it, each resident
/// here, by its `place`, as a `Resident`, and its going the fact of nothing. Who owns what is read
/// there. Who is here is so only while town runs: these facts are never logged, and one an old log
/// holds is passed over (`logged`).
pub const RESIDENTS: &str = "town/residents";

/// Whether a fact of `kind` is kept in the log: all but `RESIDENTS` and what is under it.
fn logged(kind: &str) -> bool {
    kind.strip_prefix(RESIDENTS).is_none_or(|rest| !rest.is_empty() && !rest.starts_with('/'))
}

/// Whether `kind` is a resident's, under `RESIDENTS`: what the owners are read from.
fn resident(kind: &str) -> bool {
    kind.strip_prefix(RESIDENTS).is_some_and(|rest| rest.starts_with('/'))
}

/// Whether a kind is town's own: `town` and what is under it, where town says what it is and what
/// failed, and is asked to work, wait, join, connect and open. Only what town runs may say or own
/// one; what came to the square may not.
pub fn towns(kind: &str) -> bool {
    let kind = place(kind);
    kind == "town" || kind.starts_with("town/")
}

/// How much of the past the log keeps after the present: all `past()` reads.
const KEEP: u64 = 128 * 1024;
/// How much the log may grow before it is compacted again, at least: as much as it held after it
/// was last, if that is more, so a big present is not written again for each little that comes.
const GROWTH: u64 = 1024 * 1024;

/// Every word said, to each who listens, in order: a queue each, of what it hears. Within town a
/// queue has no bound, so none there misses a word. A resident's, which may stop being read, has
/// one: what comes past it is dropped, and said, once each time it falls behind. A queue is let go
/// as its listener ends.
#[derive(Default)]
pub struct Words(RefCell<Vec<Queue>>);

/// The queue of one who listens: what it hears, and, for a resident, how many words may wait for
/// it, and who it is, to say so when more are dropped.
struct Queue {
    to: mpsc::UnboundedSender<Word>,
    hears: Hears,
    most: Option<(usize, Resident)>,
    state: Rc<State>,
}

/// What one who listens hears, as it is now: a resident says it after it begins to listen.
type Hears = Rc<RefCell<Box<dyn Fn(&Town, &Word) -> bool>>>;

/// What a queue and the one who reads it both know: how many words wait; whether it fell behind
/// since it last read; whether it is `told`, before which its falling behind is not said; and
/// whether more came before it was than may wait, so what came after was dropped, for `told` to
/// say.
#[derive(Default)]
struct State {
    waiting: Cell<usize>,
    behind: Cell<bool>,
    told: Cell<bool>,
    lost: Cell<bool>,
}

/// The words one listens to, from when it began, as many as wait.
pub struct Listening {
    from: mpsc::UnboundedReceiver<Word>,
    hears: Hears,
    state: Rc<State>,
    /// what waited as it said what it hears, and that it hears: read first
    kept: VecDeque<Word>,
}

impl Listening {
    /// Hear, from now, what `hears` hears, and of what waits, only that: as a resident says what
    /// it is.
    pub fn hear(&mut self, town: &Town, hears: impl Fn(&Town, &Word) -> bool + 'static) {
        while let Ok(w) = self.from.try_recv() {
            if hears(town, &w) {
                self.kept.push_back(w);
            }
        }
        *self.hears.borrow_mut() = Box::new(hears);
        self.state.waiting.set(self.kept.len());
    }

    /// Told from now, once greeted with the present: falling behind after is said. Of what waits,
    /// each fact goes, as the present holds it or one after it; what else waits stays. Whether more
    /// came than could wait, so some may be missed, for the caller to say.
    pub fn told(&mut self) -> bool {
        let lost = self.state.lost.replace(false);
        while let Ok(w) = self.from.try_recv() {
            self.kept.push_back(w);
        }
        // a fact kept is in the present it is greeted with; what else was kept it heard, though
        // more after it may be missed
        self.kept.retain(|w| !w.is_fact());
        self.state.waiting.set(self.kept.len());
        self.state.told.set(true);
        lost
    }

    /// the next word, once it is said; nothing once the town is gone
    pub async fn recv(&mut self) -> Option<Word> {
        let w = match self.kept.pop_front() {
            Some(w) => w,
            None => self.from.recv().await?,
        };
        Some(self.read(w))
    }

    /// the next word, if one waits
    pub fn try_recv(&mut self) -> Result<Word, mpsc::error::TryRecvError> {
        let w = match self.kept.pop_front() {
            Some(w) => w,
            None => self.from.try_recv()?,
        };
        Ok(self.read(w))
    }

    /// a word read: one fewer waits; and once none does, it is no longer behind
    fn read(&self, w: Word) -> Word {
        let waiting = self.state.waiting.get().saturating_sub(1);
        self.state.waiting.set(waiting);
        // caught up: what falls behind after is said again, once
        if waiting == 0 {
            self.state.behind.set(false);
        }
        w
    }
}

impl Words {
    /// listen from now: every word said after, in order. Only tests listen so; public for those in
    /// `tests/`.
    pub fn listen(&self) -> Listening {
        self.hear(|_, _| true, None)
    }

    /// listen from now to what `hears` hears, in order; with `most`, only so many wait, and when
    /// more come, who listens is said to fall behind
    pub fn hear(
        &self,
        hears: impl Fn(&Town, &Word) -> bool + 'static,
        most: Option<(usize, Resident)>,
    ) -> Listening {
        let (to, from) = mpsc::unbounded_channel();
        let state = Rc::new(State { told: Cell::new(true), ..State::default() });
        let hears: Hears = Rc::new(RefCell::new(Box::new(hears)));
        self.0.borrow_mut().push(Queue { to, hears: hears.clone(), most, state: state.clone() });
        Listening { from, hears, state, kept: Default::default() }
    }

    /// listen from now to every word, until it says what it hears (`Listening::hear`), as one comes
    /// before it says what it is; quietly, until it is told (`Listening::told`): if more come than
    /// may wait, what waits stays, and what comes after is dropped
    pub fn coming(&self, most: usize, who: &Resident) -> Listening {
        // who it is, to say it falls behind: no more of it than that
        let who = Resident {
            id: who.id,
            name: who.name.clone(),
            square: who.square,
            ..Resident::default()
        };
        let listening = self.hear(|_, _| true, Some((most, who)));
        listening.state.told.set(false);
        listening
    }

    /// a word, to each who hears it; one gone is let go, and one who falls behind is said, once, as
    /// an error
    fn send(&self, town: &Town, w: Word) {
        let mut behind = Vec::new();
        self.0.borrow_mut().retain(|q| {
            if q.to.is_closed() {
                return false;
            }
            if !(q.hears.borrow())(town, &w) {
                return true;
            }
            let state = &q.state;
            if let Some((_, who)) = q.most.as_ref().filter(|(most, _)| state.waiting.get() >= *most)
            {
                match state.told.get() {
                    true if !state.behind.replace(true) => behind.push(who.clone()),
                    true => {}
                    false => state.lost.set(true),
                }
                return true;
            }
            state.waiting.set(state.waiting.get() + 1);
            q.to.send(w.clone()).is_ok()
        });
        for who in behind {
            town.error(&who, None, "is not reading: words dropped".into());
        }
    }
}

/// A resident here: while this lives, its fact is in the present, and it owns what that says. As
/// this ends, however, the fact of nothing is said if the fact there is still its own: an ended
/// task lets go only when next run, and another may have come to its place by then. It holds its
/// town weakly, as the town may hold it.
pub struct Here {
    town: Weak<Town>,
    kind: Option<String>,
    id: u64,
}

impl Here {
    /// here on, as the resident that speaks for it keeps it: it goes as that one goes
    fn stays(mut self) {
        self.kind = None;
    }
}

impl Drop for Here {
    /// it goes, and those it spoke for with it, `Town::gone`
    fn drop(&mut self) {
        let Some(kind) = self.kind.take() else { return };
        let Some(town) = self.town.upgrade() else { return };
        let mine = town.present.borrow().get(&kind).is_some_and(|b| id(b) == Some(self.id));
        if mine {
            town.gone(&kind, self.id);
        }
    }
}

/// The number of the resident that speaks for the one whose fact this is, if one does.
fn spoken_by(body: &Body) -> Option<u64> {
    match body.part("by") {
        Some(Parts::Number(n)) => n.parse().ok(),
        _ => None,
    }
}

/// Whether a resident's fact says it is the language `name`.
fn speaks(body: &Body, name: &str) -> bool {
    body.part("languages").and_then(|l| l.get(name)).is_some()
}

/// The languages a resident's fact says it is, each a grammar by its name.
fn spoken(body: &Body) -> &[(String, Parts)] {
    match body.part("languages") {
        Some(Parts::Map(all)) => all,
        _ => &[],
    }
}

/// The number of the resident a fact under `RESIDENTS` says.
fn id(body: &Body) -> Option<u64> {
    match body.part("id") {
        Some(Parts::Number(n)) => n.parse().ok(),
        _ => None,
    }
}

/// One who owns a kind, as the present says: the kind as it names it, the number of who owns it,
/// and whether it came to the square. Read from the residents' facts each time one changes, and by
/// nothing else.
struct Owner {
    kind: String,
    id: u64,
    square: bool,
}

pub struct Town {
    /// kind → its latest present body
    pub present: RefCell<BTreeMap<String, Body>>,
    /// every word, to all who listen
    pub words: Words,
    /// the fact log this town folds as it starts, and appends to
    pub log: PathBuf,
    /// what has been appended to the log since it was last compacted, and how big it was after
    grown: Cell<u64>,
    compacted: Cell<u64>,
    /// what runs for each resident, by its number, its listening, its program or its join: ended
    /// as it goes, or the town opens again. Each holds the town while it runs
    pub tasks: RefCell<Vec<(u64, tokio::task::JoinHandle<()>)>>,
    /// kind → what town has joined or connected to there, ended when the kind is joined again
    pub joins: RefCell<HashMap<String, Joined>>,
    /// itself, to begin what is wanted of it as it is asked
    me: Weak<Town>,
    /// the number the next to come is given
    next: Cell<u64>,
    /// the process group of each program running, ended when the town opens again or stops
    pub groups: RefCell<HashSet<crate::unix::Pid>>,
    /// how bytes are read as parts, and parts written as bytes, by name
    pub languages: crate::language::Languages,
    /// who owns what, read from the residents' facts as each changes (`talk`): the present is the
    /// one truth, read once
    owners: RefCell<Vec<Owner>>,
    /// what the town keeps to open again as it is asked, once it is kept (`residents::keep`)
    pub opening: RefCell<Option<Rc<crate::residents::Opening>>>,
    /// the world its processes run in, each var to its value: a command, a join, a program. The
    /// process's own, unless it is given another
    pub world: BTreeMap<OsString, OsString>,
    /// who was told another's `town/error` last, by number: what it says now may answer it
    /// (`failed`)
    answering: RefCell<HashSet<u64>>,
    /// what the residents of the last opening owned, by the kind of their fact: held against one at
    /// the square until one comes there again, or the one it is under (`come`), or none will
    /// (`hold_only`), as a program says what it is only once it runs
    held: RefCell<Vec<(String, Owner)>>,
}

impl Resident {
    /// Whether it hears, owns, stands at and is nothing, so has no fact.
    pub fn nothing(&self) -> bool {
        self.kinds.is_empty()
            && self.own.is_empty()
            && self.at.is_none()
            && self.languages.is_empty()
    }

    /// town itself: number 0, here from the start
    pub fn town() -> Resident {
        Resident { name: "town".into(), ..Resident::default() }
    }

    /// Where its fact is, under `RESIDENTS`: one at the square by its number, `square/<id>`; a
    /// join by its kind; a file by its whole path, its leading `/` the separator; any other by its
    /// number.
    pub fn place(&self) -> String {
        if let Some(placed) = &self.placed {
            return placed.clone();
        }
        match (&self.at, &self.file) {
            _ if self.square => format!("square/{}", self.id),
            (Some(at), _) => at.clone(),
            (None, Some(file)) => file.to_string_lossy().trim_start_matches('/').into(),
            (None, None) => self.id.to_string(),
        }
    }
}

impl Town {
    /// A town that folds the log at `log` into its present, and appends to it: town's own,
    /// `paths::past()`, or a test's. It runs in the process's world.
    pub fn with_log(log: PathBuf) -> Rc<Town> {
        Town::in_world(log, std::env::vars_os().collect())
    }

    /// A town, as `with_log`, that runs in `world`.
    pub fn in_world(log: PathBuf, world: BTreeMap<OsString, OsString>) -> Rc<Town> {
        // the present is the fold of the past: a line that is no word is passed over
        let present = fold(said(log::all(&log)));
        Rc::new_cyclic(|me| Town {
            me: me.clone(),
            present: RefCell::new(present),
            words: Words::default(),
            log,
            grown: Cell::new(0),
            compacted: Cell::new(0),
            tasks: RefCell::new(Vec::new()),
            joins: RefCell::new(HashMap::new()),
            next: Cell::new(0),
            groups: RefCell::new(HashSet::new()),
            languages: Default::default(),
            owners: Default::default(),
            opening: Default::default(),
            world,
            answering: Default::default(),
            held: Default::default(),
        })
    }

    /// One come to town, by the name it goes by: given a number no other has.
    pub fn who(&self, name: &str, square: bool) -> Resident {
        self.next.set(self.next.get() + 1);
        Resident { id: self.next.get(), name: name.to_string(), square, ..Resident::default() }
    }

    /// `who` comes, as what it says it is: its fact, at its `place`, unless it hears, owns, stands
    /// at and is nothing. It owns all it says, or none (`may_own`).
    pub fn come(self: &Rc<Self>, who: &Resident) -> Result<Here, String> {
        self.may_come(who, None)?;
        // come again, what was held for it and under it goes, as what it asks for again it asks
        // from here; but a Lua file's, which asks as it hears its greeting (`residents::holding`)
        let kind = format!("{RESIDENTS}/{}", who.place());
        if !who.lua {
            let within = |at: &str| at.strip_prefix(&kind).is_some_and(|r| r.starts_with('/'));
            self.held.borrow_mut().retain(|(at, _)| *at != kind && !within(at));
        }
        if who.nothing() {
            return Ok(Here { town: Rc::downgrade(self), kind: None, id: who.id });
        }
        let body = Body::of(serde_json::to_value(who).map(Parts::from).map_err(|e| e.to_string())?);
        // the languages it is, as its fact says them: none json, town's own, nor another's; each a
        // grammar, made once, here
        let mut made = Vec::new();
        for (name, grammar) in spoken(&body) {
            if name == "json" {
                return Err("json is town's own".into());
            }
            if who.square {
                return Err(format!("`{name}`: what came to the square is no language"));
            }
            if self.residents().values().any(|r| id(r) != Some(who.id) && speaks(r, name)) {
                return Err(format!("another is `{name}`"));
            }
            made.push(crate::language::Grammar::of(grammar).map_err(|e| format!("{name}: {e}"))?);
        }
        for ((name, grammar), made) in spoken(&body).iter().zip(made) {
            self.languages.made(&kind, name, grammar, made);
        }
        self.talk(Word::new(kind.as_str(), Tense::Present, body));
        Ok(Here { town: Rc::downgrade(self), kind: Some(kind), id: who.id })
    }

    /// Whether `who` may come, but for the resident numbered `besides`, as one it takes the place
    /// of: to a place none else is at, owning what it may (`may_own`).
    pub fn may_come(&self, who: &Resident, besides: Option<u64>) -> Result<(), String> {
        // one place, one resident: none comes where another is, as a join at `square/3`
        let kind = format!("{RESIDENTS}/{}", who.place());
        let there = self.present.borrow().get(&kind).and_then(id);
        if there.is_some_and(|it| it != who.id && Some(it) != besides) {
            return Err(format!("another is at `{}`", who.place()));
        }
        self.may_own(who, besides)
    }

    /// Whether `who` may own what it says it owns, but for what the resident numbered `besides`
    /// owns, as one it takes the place of: not every kind; not town's own, from the square; not
    /// what another owns, however either names it; and from the square, nothing within another's,
    /// whichever came first: one at the square that owns `box/cat` keeps a join at `box` away.
    fn may_own(&self, who: &Resident, besides: Option<u64>) -> Result<(), String> {
        let own = &who.own;
        if own.iter().any(|kind| place(kind).is_empty()) {
            return Err("none owns every kind: that would silence the town".into());
        }
        if let Some(kind) = own.iter().find(|kind| who.square && towns(kind)) {
            return Err(format!("`{kind}` is town's own"));
        }
        let (owners, held) = (self.owners.borrow(), self.held.borrow());
        let other = |o: &&Owner| o.id != who.id && Some(o.id) != besides;
        let mut others: Vec<&Owner> = owners.iter().filter(other).collect();
        // what the last opening's residents owned is held from the square until they come again
        others.extend(held.iter().map(|(_, o)| o).filter(|_| who.square));
        let taken: HashSet<&str> = others.iter().map(|o| place(&o.kind)).collect();
        if let Some(kind) = own.iter().find(|kind| taken.contains(&place(kind))) {
            return Err(format!("another owns `{kind}`"));
        }
        let within = |kind: &String| others.iter().any(|o| hears(&o.kind, place(kind)));
        if let Some(kind) = own.iter().find(|kind| who.square && within(kind)) {
            return Err(format!("another owns what is around `{kind}`"));
        }
        let square = others.iter().filter(|o| o.square && !who.square);
        if let Some(o) = square.into_iter().find(|o| own.iter().any(|k| hears(k, place(&o.kind)))) {
            return Err(format!("one at the square owns `{}`, within what it would own", o.kind));
        }
        Ok(())
    }

    /// The place of the resident numbered `who`, if it is here.
    pub(crate) fn place_of(&self, who: u64) -> Option<String> {
        let mine = |(kind, body): (String, Body)| (id(&body) == Some(who)).then_some(kind);
        let kind = self.residents().into_iter().find_map(mine)?;
        kind.strip_prefix(&format!("{RESIDENTS}/")).map(String::from)
    }

    /// Who is here, by place: the facts under `RESIDENTS`.
    fn residents(&self) -> BTreeMap<String, Body> {
        self.facts(&format!("{RESIDENTS}/"))
    }

    /// Who owns what, as the present says, read again as the resident's fact at `kind` changes,
    /// before any hears it; and the languages it is, if it is or `was` any, so a language is spoken
    /// from the moment it is said, and only those of the one that changed are made again.
    fn own(&self, kind: &str, was: bool) {
        let mut owners = Vec::new();
        for body in self.residents().into_values() {
            let Some(id) = id(&body) else { continue };
            let square = body.part("square").is_some();
            let own = body.part("own").and_then(Parts::list).into_iter().flatten();
            let owner = |kind: &str| Owner { kind: kind.into(), id, square };
            owners.extend(own.filter_map(Parts::text).map(owner));
        }
        *self.owners.borrow_mut() = owners;
        // come again, what was held there is its own, or another's
        if self.present.borrow().contains_key(kind) {
            self.held.borrow_mut().retain(|(at, _)| at != kind);
        }
        let is = self.present.borrow().get(kind).map(|b| spoken(b).to_vec()).unwrap_or_default();
        if was || !is.is_empty() {
            self.languages.are(kind, &is);
        }
    }

    /// The resident at `kind`, numbered `who`, goes, and those it spoke for with it, and those they
    /// spoke for, however deep; not another under its place, as a join at `box/cat` is under
    /// one at `box`, and speaks for itself.
    pub(crate) fn gone(&self, kind: &str, who: u64) {
        let under = self.facts(&format!("{kind}/"));
        let (mut going, mut ids, mut gone) = (vec![kind.to_string()], vec![who], vec![who]);
        while let Some(by) = ids.pop() {
            for (kind, body) in &under {
                if spoken_by(body) == Some(by) {
                    going.push(kind.clone());
                    ids.extend(id(body));
                    gone.extend(id(body));
                }
            }
        }
        // what runs for them ends, their joins with it: taken out, then let go, so what their
        // going says finds both free to look at. Not the task that runs this, as a program's run
        // that goes as it ends: it ends itself, and hears the last of what it said
        let running = tokio::task::try_id();
        let (tasks, joins): (Vec<_>, Vec<_>) = {
            let mut tasks = self.tasks.borrow_mut();
            let mut joins = self.joins.borrow_mut();
            let ends = |(id, task): &mut (u64, tokio::task::JoinHandle<()>)| {
                gone.contains(id) && Some(task.id()) != running
            };
            let tasks = tasks.extract_if(.., ends).collect();
            let joins = joins.extract_if(|_, j| gone.contains(&j.who.id)).collect();
            (tasks, joins)
        };
        tasks.into_iter().for_each(|(_, task)| task.abort());
        drop(joins);
        // the fact of nothing, only where a fact is: a join let go has said its own
        for kind in going.into_iter().rev() {
            if self.present.borrow().contains_key(&kind) {
                self.talk(Word::new(kind, Tense::Present, Body::none()));
            }
        }
    }

    /// What runs for the resident numbered `who`: ended as it goes (`gone`), or the town opens
    /// again.
    pub fn runs(&self, who: u64, task: tokio::task::JoinHandle<()>) {
        let mut tasks = self.tasks.borrow_mut();
        tasks.retain(|(_, t)| !t.is_finished());
        tasks.push((who, task));
    }

    /// The resident whose fact is at `kind`, numbered `who`, goes to come again, as its file is
    /// saved: what it and those it spoke for owned is held from the square until they come again.
    pub(crate) fn leaves(&self, kind: &str, who: u64) {
        let under = |k: &String| k == kind || k.starts_with(&format!("{kind}/"));
        let leaving: Vec<(String, Body)> = self
            .residents()
            .into_iter()
            .filter(|(k, b)| under(k) && b.part("square").is_none())
            .collect();
        self.hold(&leaving);
        self.gone(kind, who);
    }

    /// What residents going to come again owned, held from the square until they come (`held`).
    fn hold(&self, going: &[(String, Body)]) {
        let owned: Vec<(String, Owner)> = going
            .iter()
            .flat_map(|(kind, body)| {
                let own = body.part("own").and_then(Parts::list).into_iter().flatten();
                let owner = |own: &str| Owner {
                    kind: own.into(),
                    id: id(body).unwrap_or(0),
                    square: false,
                };
                own.filter_map(Parts::text).map(move |own| (kind.clone(), owner(own)))
            })
            .collect();
        self.held.borrow_mut().extend(owned);
    }

    /// One `who` speaks for, at `name` under its place: what it is in `body`, checked as any
    /// resident is as it comes; or, of nothing or as nothing, `{}`, gone. It stands as `who`
    /// stands, and goes as it goes.
    fn under(&self, who: &Resident, name: &str, body: &Body) -> Result<(), String> {
        // one that says nothing of itself has no place to speak from, and none to go from
        let mine = format!("{RESIDENTS}/{}", who.place());
        if self.present.borrow().get(&mine).is_none_or(|b| id(b) != Some(who.id)) {
            return Err("one that says nothing of itself speaks for none".into());
        }
        let place = format!("{}/{name}", who.place());
        let kind = format!("{RESIDENTS}/{place}");
        // one it spoke for before, at that place: said again, it is the same one
        let was =
            self.present.borrow().get(&kind).filter(|b| spoken_by(b) == Some(who.id)).cloned();
        let said: Resident = match body.is_none() {
            true => Resident::default(),
            false => {
                let value = serde_json::to_value(body.parts()).map_err(|e| e.to_string())?;
                serde_json::from_value(value).map_err(|e| e.to_string())?
            }
        };
        // of nothing, or as nothing, `{}`, it has no fact: the one it had goes
        if said.nothing() {
            if let Some(gone) = was.as_ref().and_then(id) {
                self.gone(&kind, gone);
            }
            return Ok(());
        }
        let me = self.me.upgrade().ok_or("the town is gone")?;
        let name = name.rsplit('/').next().unwrap_or(name);
        let id = was.as_ref().and_then(id).unwrap_or_else(|| self.who(name, who.square).id);
        let by = Some(who.id);
        let one = Resident { square: who.square, placed: Some(place), by, ..said };
        let one = Resident { id, name: name.into(), ..one };
        me.come(&one)?.stays();
        Ok(())
    }

    /// The resident `by` names, under the place of `who`, who speaks for it: as its fact
    /// says it is, standing as `who` stands, `who` its speaker.
    pub fn spoken_for(&self, who: &Resident, by: &str) -> Result<Resident, String> {
        let kind = format!("{RESIDENTS}/{}/{by}", who.place());
        let fact = self.present.borrow().get(&kind).cloned();
        let fact = fact.filter(|f| spoken_by(f) == Some(who.id));
        let fact = fact.ok_or(format!("it speaks for no resident `{by}`"))?;
        let id = id(&fact).ok_or(format!("it speaks for no resident `{by}`"))?;
        let placed = Some(format!("{}/{by}", who.place()));
        let one =
            Resident { id, name: by.into(), square: who.square, placed, ..Resident::default() };
        Ok(Resident { by: Some(who.id), ..one })
    }

    /// The town opens again: all that ran for it ends, residents, programs and joins, and their
    /// facts go here, at once, since an ended task lets go only when next run. What came to the
    /// square stays; what the others owned is held from it until they come again (`held`).
    pub fn reopen(&self) {
        self.tasks.borrow_mut().drain(..).for_each(|(_, h)| h.abort());
        // their programs now, not as their tasks are let go: a new run must not meet the old
        self.end_groups();
        drop(self.joins.take());
        let gone: Vec<(String, Body)> = self
            .residents()
            .into_iter()
            .filter(|(_, body)| body.part("square").is_none())
            .collect();
        self.hold(&gone);
        for (kind, _) in gone {
            self.talk(Word::new(kind, Tense::Present, Body::none()));
        }
    }

    /// What is held from the last opening, of those whose fact would be at a kind `still` keeps:
    /// the rest is let go, as none will come there.
    pub(crate) fn hold_only(&self, still: impl Fn(&str) -> bool) {
        self.held.borrow_mut().retain(|(kind, _)| still(kind));
    }

    /// The resident numbered `who` is told `w`: whether it is another's `town/error`, so what it
    /// says now may answer it (`failed`); and, with none, it is told no more.
    pub(crate) fn telling(&self, who: u64, w: Option<&Word>) {
        let mut answering = self.answering.borrow_mut();
        match w.is_some_and(|w| w.kind == ERROR && w.by != who) {
            true => answering.insert(who),
            false => answering.remove(&who),
        };
    }

    /// End the group of every program running.
    pub fn end_groups(&self) {
        self.groups.borrow_mut().drain().for_each(crate::unix::end_group);
    }

    /// Something failed: said as a past `town/error` word, `{ from, heard, message }`, so whoever
    /// listens can see it, and printed for the log. `heard` is the kind of word in hand when it
    /// failed, if there was one.
    ///
    /// An error of a word in hand is only printed if it is of one hearing a `town/error`: it failed
    /// at one, or what it says while it hears one is refused or no word. Two who hear `town/error`
    /// would make each other's errors without end. Who hears one is who was told another's last
    /// (`telling`): a Lua resident while its `listen` hears it, a program until it is told another.
    pub fn error(&self, from: &Resident, heard: Option<&str>, message: String) {
        self.failed(from, message, heard.map(|kind| ("heard", kind)));
    }

    /// Something said that is no word: a `town/error`, with what was `said`, as far as `cut` keeps
    /// it, so whoever said it can know it for theirs.
    pub fn misheard(&self, from: &Resident, said: &str, message: String) {
        self.failed(from, message, Some(("said", &cut(said))));
    }

    /// An error by the one it is of, its number on the word; but of one spoken for, by its
    /// speaker, who said the word and is told of it (`Resident::hears`), the name its own.
    fn failed(&self, from: &Resident, message: String, also: Option<(&str, &str)>) {
        eprintln!("town: {}: {message}", from.name);
        let by = from.by.unwrap_or(from.id);
        let answering = also.is_some() && self.answering.borrow().contains(&by);
        if also == Some(("heard", ERROR)) || answering {
            return;
        }
        let mut body = Parts::map([("from", from.name.as_str()), ("message", &message)]);
        if let Some((name, part)) = also {
            body.set(name, part);
        }
        self.talk(Word { by, ..Word::new(ERROR, Tense::Past, Body::of(body)) });
    }

    /// May `who` say this word? Not town's own, from the square; and not what is, or was, of a kind
    /// another owns, as `hears` reads it. What is wanted of a kind, anyone may say: that is how one
    /// asks it. The nearest owner counts, so a join at `box/cat` says what is there, though one at
    /// `box` owns what is under it; each place has one owner, so the nearest is one.
    pub fn may(&self, w: &Word, who: &Resident) -> Result<(), String> {
        // the paths it was opened with are town's alone to say, as it opens
        if w.kind == RESIDENTS {
            return Err(format!("`{RESIDENTS}` is town's to say"));
        }
        if who.square && towns(&w.kind) {
            return Err(format!("`{}` is town's own", w.kind));
        }
        // to want what is joined is to have town run it: town's own, as `town/join` is
        if who.square && w.tense == Tense::Future && self.joined(&w.kind).is_some() {
            return Err(format!("`{}` is joined: what is wanted of it is town's own", w.kind));
        }
        // what is wanted of a kind is told to what stands there, and no nearer join: it says what
        // is, and what was, and may want what a nearer one is
        if let Some(at) = who.at.as_ref() {
            if w.tense == Tense::Future && self.joined(&w.kind).as_ref() == Some(at) {
                return Err(format!("it is {at}: a future word there is told to it"));
            }
        }
        if w.tense == Tense::Future {
            return Ok(());
        }
        let owners = self.owners.borrow();
        let owners: Vec<_> = owners.iter().filter(|o| hears(&o.kind, &w.kind)).collect();
        let near = |o: &Owner| place(&o.kind).len();
        let Some(nearest) = owners.iter().map(|o| near(o)).max() else { return Ok(()) };
        match owners.iter().any(|o| near(o) == nearest && o.id == who.id) {
            true => Ok(()),
            false => Err("refused: another owns it".into()),
        }
    }

    /// The kind joined nearest `kind`: it, or the nearest above it.
    pub fn joined(&self, kind: &str) -> Option<String> {
        let joins = self.joins.borrow();
        let mut at = kind;
        loop {
            if joins.contains_key(at) {
                return Some(at.to_string());
            }
            at = &at[..at.rfind('/')?];
        }
    }

    /// The one door a word comes in by, from whoever says it: said in town if `who` may say it,
    /// stamped with the moment it is, whatever `when` it came with; and if not, said back as a
    /// `town/error` from it.
    pub fn said(&self, w: Word, who: &Resident) {
        // a resident under its own place, placed from where it stands, as a join's words are
        // from its kind: it says that one is there, or is no longer
        let mine = format!("{RESIDENTS}/");
        if let Some(name) = w.kind.strip_prefix(&mine).filter(|_| w.tense == Tense::Present) {
            let under = match who.lua {
                true => Err("a Lua resident speaks for none: its words have no `by`".into()),
                false => self.under(who, name, &w.body),
            };
            if let Err(why) = under {
                self.error(who, Some(&w.kind), why);
            }
            return;
        }
        match self.may(&w, who) {
            Ok(()) => self.talk(Word { by: who.id, when: 0, ..w }),
            Err(why) => self.error(who, Some(&w.kind), why),
        }
    }

    /// Talk a word: a fact folds into the present and is logged, and every word is sent on. A fact
    /// of nothing is logged too, so the present rebuilt from the log forgets as this one did. Who
    /// is here, under `RESIDENTS`, is kept in the present alone, and who owns what is read
    /// from it.
    pub fn talk(&self, mut word: Word) {
        if !is_kind(&word.kind) {
            let message = format!("`{}` is no kind: {}", word.kind, KIND);
            return self.error(&Resident::town(), None, message);
        }
        if word.when == 0 {
            word.when = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0);
        }
        let was = resident(&word.kind)
            && self.present.borrow().get(&word.kind).is_some_and(|b| !spoken(b).is_empty());
        let kept = keep(&mut self.present.borrow_mut(), &word);
        if kept && resident(&word.kind) {
            self.own(&word.kind, was);
        }
        if kept && logged(&word.kind) {
            self.grown.set(self.grown.get() + log::append(&self.log, &word));
            if self.grown.get() > GROWTH.max(self.compacted.get()) {
                self.compact();
            }
        }
        // what is wanted of town begins as it is asked for: a join hears what is wanted of it after
        let asked =
            (word.tense == Tense::Future).then(|| word.kind.strip_prefix("town/")).flatten();
        let asked = asked.map(|verb| (verb.to_string(), word.body.clone(), word.by));
        self.words.send(self, word);
        if let (Some((verb, body, by)), Some(me)) = (asked, self.me.upgrade()) {
            crate::work::begin(&me, &verb, body, by);
        }
    }

    /// The language `name`, once it is said: now, if it has been, or else when it is, telling
    /// `waits`, once, that it waits for it, since a program says its language as it runs, after
    /// the files beside it have opened. Nothing if the town stops first.
    pub async fn language(
        &self,
        name: &str,
        mut waits: impl AsyncFnMut(&str),
    ) -> Option<Rc<crate::language::Language>> {
        let mut words = self.words.hear(|_, w| resident(&w.kind), None);
        let mut told = false;
        loop {
            if let Ok(l) = self.languages.get(name) {
                return Some(l);
            }
            if !std::mem::replace(&mut told, true) {
                waits(&format!("no language is called {name} yet: it waits for one")).await;
            }
            words.recv().await?;
        }
    }

    /// Rewrite the log down to the present facts and a bounded recent tail: as town starts, and
    /// whenever it has grown by `GROWTH`, or by what it held after, since. See `log::compact`.
    pub fn compact(&self) {
        let present: Vec<Word> =
            self.present_words().into_iter().filter(|w| logged(&w.kind)).collect();
        log::compact(&self.log, &present, KEEP);
        self.grown.set(0);
        self.compacted.set(std::fs::metadata(&self.log).map_or(0, |m| m.len()));
    }

    /// The present as words, as the log keeps them.
    pub fn present_words(&self) -> Vec<Word> {
        self.present
            .borrow()
            .iter()
            .map(|(k, body)| Word::new(k.clone(), Tense::Present, body.clone()))
            .collect()
    }

    /// The facts a kind hears, by kind: `nvim` alone for `nvim`; `nvim/1` and `nvim/2/a` for
    /// `nvim/`; all three for `/nvim`; every one for `/`.
    pub fn facts(&self, kind: &str) -> BTreeMap<String, Body> {
        if is_kind(kind) {
            let one = self.present.borrow().get(kind).cloned();
            return one.map(|body| (kind.to_string(), body)).into_iter().collect();
        }
        let from = place(kind);
        self.present
            .borrow()
            .range(from.to_string()..)
            .take_while(|(k, _)| k.starts_with(from))
            .filter(|(k, _)| hears(kind, k))
            .map(|(k, body)| (k.clone(), body.clone()))
            .collect()
    }

    /// The past a kind hears, by kind: each kind's facts in the order they were said, but for the
    /// times there was none. It reads only the tail of the log, so its cost is bounded however
    /// big the log grows.
    pub fn past(&self, kind: &str) -> BTreeMap<String, Vec<Body>> {
        let mut past = BTreeMap::<String, Vec<Body>>::new();
        let said = said(log::tail(&self.log, KEEP));
        for w in said.filter(|w| w.is_fact() && !w.body.is_none() && hears(kind, &w.kind)) {
            past.entry(w.kind).or_default().push(w.body);
        }
        past
    }
}

/// How much of what is said a `town/error` holds of it: a line that is no word, a program's
/// stderr. One may say up to `MOST` that is no word, and each error waits for whoever hears it.
const SAID: usize = 1024;

/// What is said, as far as `SAID`, and how much more there was, so an error costs little however
/// much was said.
pub(crate) fn cut(said: &str) -> std::borrow::Cow<'_, str> {
    if said.len() <= SAID {
        return said.into();
    }
    let at = (0..=SAID).rev().find(|&n| said.is_char_boundary(n)).unwrap_or(0);
    format!("{}… and {} more bytes", &said[..at], said.len() - at).into()
}

/// The words of the log's lines: a line that is no word is passed over, and so is a fact not
/// logged, as an old log may hold one.
fn said(lines: impl Iterator<Item = String>) -> impl Iterator<Item = Word> {
    lines.filter_map(|l| read(&l).ok()).filter(|w| logged(&w.kind))
}

/// The present, folded from the past: the last fact of each kind.
pub fn fold(words: impl Iterator<Item = Word>) -> BTreeMap<String, Body> {
    let mut present = BTreeMap::new();
    for w in words {
        keep(&mut present, &w);
    }
    present
}

/// A fact sets its kind in the present. A fact of nothing, one with no body, is the fact that
/// there is none: its kind leaves the present. Whether it was a fact, so kept.
fn keep(present: &mut BTreeMap<String, Body>, w: &Word) -> bool {
    if !w.is_fact() {
        return false;
    }
    match w.body.is_none() {
        true => present.remove(&w.kind),
        false => present.insert(w.kind.clone(), w.body.clone()),
    };
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::scratch;

    // the tail of the log holds the whole last line, wherever its window begins, inside a char
    // of many bytes too
    #[test]
    fn a_tail_begun_inside_a_char_holds_the_last_line() {
        let dir = scratch();
        let path = dir.log();
        let content = "\
{\"kind\":\"a\",\"tense\":\"present\",\"body\":{\"x\":\"aaaaaaaa\"}}
{\"kind\":\"b\",\"tense\":\"present\",\"body\":{\"x\":\"héllo-世界\"}}
{\"kind\":\"c\",\"tense\":\"present\",\"body\":{\"x\":\"zzzz\"}}\n";
        std::fs::write(&path, content).unwrap();
        let last = content.rfind("{\"kind\":\"c\"").unwrap();
        // every window that begins before the last line, some inside 世 or 界
        for w in (content.len() - last + 1) as u64..=content.len() as u64 {
            let got: Vec<String> = crate::log::tail(&path, w).collect();
            assert!(got.iter().any(|l| l.contains("zzzz")), "window {w} lost the last line");
        }
    }

    // a fact of nothing takes its kind out of the present, here and in a town rebuilt from the
    // log, after compaction too; and `past` passes over it
    #[test]
    fn a_fact_of_nothing_forgets() {
        let dir = scratch();
        let path = dir.log();
        let town = Town::with_log(path.clone());
        town.talk(Word::new("song", Tense::Present, Body::of(serde_json::json!({ "title": "a" }))));
        town.talk(Word::new(
            "theme",
            Tense::Present,
            Body::of(serde_json::json!({ "bg": "#111" })),
        ));
        town.talk(Word::new("song", Tense::Present, Body::none()));
        assert!(!town.present.borrow().contains_key("song"));
        assert!(town.present.borrow().contains_key("theme"));
        assert!(!Town::with_log(path.clone()).present.borrow().contains_key("song"));
        town.compact();
        assert!(!Town::with_log(path.clone()).present.borrow().contains_key("song"));
        assert_eq!(town.past("song")["song"], [Parts::from(serde_json::json!({ "title": "a" }))]);
    }

    // a kind owned is said by who owns it, and by no other, until it lets go; the first to own it
    // keeps it, however another names it; town's own words are not said from outside
    #[test]
    fn a_kind_owned_is_said_by_who_owns_it() {
        let dir = scratch();
        let town = Town::with_log(dir.log());
        let (a, b, c) = (town.who("a", false), town.who("b", false), town.who("c", false));
        let mine = Word::new("mine", Tense::Past, Body::none());
        let mine_kinds = ["mine".to_string()];
        let owning = town.come(&Resident { own: mine_kinds.to_vec(), ..a.clone() }).unwrap();
        let fact = format!("{RESIDENTS}/{}", a.id);
        assert_eq!(town.facts(&fact)[&fact].value()["own"], serde_json::json!(["mine"]), "a fact");
        for taken in ["mine", "/mine", "mine/"] {
            assert!(
                town.come(&Resident { own: vec![taken.to_string()], ..b.clone() }).is_err(),
                "the first keeps it: {taken}"
            );
        }
        for every in ["/", "//", ""] {
            assert!(
                town.come(&Resident { own: vec![every.to_string()], ..c.clone() }).is_err(),
                "none owns every kind: {every:?}"
            );
        }
        assert!(town.may(&mine, &a).is_ok());
        assert!(town.may(&mine, &b).is_err() && town.may(&mine, &c).is_err(), "another owns it");
        drop(owning);
        assert!(town.facts(&fact).is_empty(), "gone, the fact of nothing");
        assert!(town.may(&mine, &c).is_ok(), "let go, anyone may say it");
        assert!(town.come(&c).is_ok() && town.residents().is_empty(), "nothing to say");
        {
            let _owning = town.come(&Resident { own: mine_kinds.to_vec(), ..b.clone() }).unwrap();
            assert!(town.may(&mine, &c).is_err(), "owned while it lasts");
        }
        assert!(town.may(&mine, &c).is_ok(), "and let go as it ends");
        let work = Word::new("town/work", Tense::Future, Body::none());
        assert!(town.may(&work, &c).is_ok(), "what town runs may ask it to work");
        let outside = town.who("square", true);
        assert!(town.may(&work, &outside).is_err(), "what came to it may not");
        let residents = Word::new(RESIDENTS, Tense::Present, Body::none());
        assert!(town.may(&residents, &outside).is_err(), "nor say what town is");
        assert!(
            town.come(&Resident { own: vec!["town/".into()], ..outside.clone() }).is_err(),
            "nor own it"
        );
        assert!(
            town.come(&Resident { own: vec!["town/work".into()], ..outside.clone() }).is_err(),
            "nor town's own"
        );
    }

    // who is here is a fact: a reopen lets go of all but what came to the square, and a town
    // started again of all, since none of them is here
    #[test]
    fn who_is_here_is_a_fact() {
        let dir = scratch();
        let log = dir.log();
        let town = Town::with_log(log.clone());
        let (inside, outside) = (town.who("a", false), town.who("square", true));
        let a = town.come(&Resident { kinds: vec!["x".into()], ..inside.clone() }).unwrap();
        let square = town.come(&Resident { kinds: vec!["x".into()], ..outside.clone() }).unwrap();
        let here = |town: &Town| town.residents().len();
        assert_eq!(here(&town), 2);
        town.reopen();
        assert_eq!(here(&town), 1, "what came to the square stays");
        drop(a);
        assert_eq!(here(&town), 1, "one gone already is not said gone again");
        assert_eq!(here(&Town::with_log(log.clone())), 0, "a town started again has none here");
        drop(square);
    }

    // a resident is where it lives: a file by its whole path, a join by its kind, one at the square
    // by its number; its fact names its number, so one let go late, after another came to its
    // place, leaves the newcomer's fact, and what it owns, as they are
    #[test]
    fn a_resident_is_where_it_lives() {
        let dir = scratch();
        let log = dir.log();
        let town = Town::with_log(log.clone());
        let mine = vec!["mine".to_string()];
        let file = |who: Resident| Resident {
            file: Some("/a/b/theme.lua".into()),
            own: mine.clone(),
            ..who
        };
        let old = file(town.who("theme", false));
        let theme = format!("{RESIDENTS}/a/b/theme.lua");
        let late = town.come(&old).unwrap();
        assert_eq!(town.facts(&theme)[&theme].value()["id"], old.id, "by its whole path");
        let joined = Resident { at: Some("notes".into()), ..town.who("notes", false) };
        let _joined = town.come(&joined).unwrap();
        assert!(town.facts(&format!("{RESIDENTS}/notes")).len() == 1, "by its kind");
        let square = Resident { kinds: vec!["x".into()], ..town.who("square", true) };
        let _square = town.come(&square).unwrap();
        let at = format!("{RESIDENTS}/square/{}", square.id);
        assert!(town.facts(&at).len() == 1, "by its number");
        town.reopen();
        let new = file(town.who("theme", false));
        let _new = town.come(&new).unwrap();
        drop(late);
        assert_eq!(town.facts(&theme)[&theme].value()["id"], new.id, "the newcomer's, still");
        let said = Word::new("mine", Tense::Past, Body::none());
        assert!(town.may(&said, &new).is_ok() && town.may(&said, &square).is_err(), "its own");
    }

    // a resident speaks for those under its place: it says one is there, placed from where it
    // stands, and that one is a resident as any is, standing as it stands; a word by it is its;
    // and as the first goes, those it speaks for go
    #[test]
    fn a_resident_speaks_for_those_under_it() {
        let dir = scratch();
        let town = Town::with_log(dir.log());
        let host = town.who("host", false);
        let here = town.come(&Resident { kinds: vec!["x".into()], ..host.clone() }).unwrap();
        let said = |kind: &str, body: serde_json::Value| {
            town.said(Word::new(kind, Tense::Present, Body::of(body)), &host)
        };
        said("town/residents/theme.lua", serde_json::json!({ "own": ["theme"] }));
        let theme = town.spoken_for(&host, "theme.lua").unwrap();
        assert_eq!(theme.place(), format!("{}/theme.lua", host.place()), "under its place");
        let fact = town.facts(&format!("{RESIDENTS}/{}", theme.place()));
        assert_eq!(fact.values().next().unwrap().value()["own"], serde_json::json!(["theme"]));
        let word = Word::new("theme", Tense::Present, Body::of("dark"));
        assert!(town.may(&word, &theme).is_ok(), "its own");
        assert!(town.may(&word, &host).is_err(), "not its speaker's");
        assert!(town.may(&Word::new("town/work", Tense::Future, Body::none()), &theme).is_ok());
        let square = town.who("square", true);
        let _sq = town.come(&Resident { kinds: vec!["x".into()], ..square.clone() }).unwrap();
        let mut errors = town.words.hear(|_, w| w.kind == ERROR, None);
        town.said(
            Word::new(
                "town/residents/a",
                Tense::Present,
                Body::of(serde_json::json!({ "kinds": ["x"] })),
            ),
            &square,
        );
        let a = town.spoken_for(&square, "a").unwrap();
        assert!(a.square, "it stands as its speaker stands");
        let mine = |own: &str| Body::of(serde_json::json!({ "own": [own] }));
        town.said(Word::new("town/residents/b", Tense::Present, mine("theme")), &host);
        assert!(errors.try_recv().is_ok(), "checked as any that comes: another owns theme");
        assert!(town.spoken_for(&host, "nobody").is_err());
        // said again, it is the same one, not another that would own what it owns
        let again = |own: &str| Word::new("town/residents/theme.lua", Tense::Present, mine(own));
        town.said(again("theme"), &host);
        assert!(errors.try_recv().is_err(), "the same one");
        assert_eq!(town.spoken_for(&host, "theme.lua").unwrap().id, theme.id);
        // one under its place that it does not speak for is not its, and stays as it goes
        let nested = Resident {
            kinds: vec!["x".into()],
            placed: Some(format!("{}/nested", host.place())),
            ..town.who("nested", false)
        };
        let _nested = town.come(&nested).unwrap();
        assert!(town.spoken_for(&host, "nested").is_err(), "it speaks for it not");
        // one it speaks for may speak for others, and they go with it, however deep
        let deeper = Word::new(
            "town/residents/x",
            Tense::Present,
            Body::of(serde_json::json!({ "kinds": ["x"] })),
        );
        town.said(deeper.clone(), &theme);
        assert!(town.spoken_for(&theme, "x").is_ok(), "spoken for by the one spoken for");
        // taken away, it goes with those it spoke for; and said again, they may come again
        let away = Word::new("town/residents/theme.lua", Tense::Present, Body::none());
        town.said(away, &host);
        assert!(town.spoken_for(&host, "theme.lua").is_err(), "taken away");
        let x = town.facts(&format!("{RESIDENTS}/{}/x", theme.place()));
        assert!(x.is_empty(), "and the one it spoke for, with it");
        town.said(again("theme"), &host);
        let theme = town.spoken_for(&host, "theme.lua").unwrap();
        town.said(deeper, &theme);
        // one that says nothing of itself speaks for none
        let quiet = town.who("quiet", false);
        let _quiet = town.come(&quiet).unwrap();
        town.said(Word::new("town/residents/q", Tense::Present, mine("q")), &quiet);
        assert!(town.spoken_for(&quiet, "q").is_err(), "it has no place to speak from");
        let _ = errors.try_recv();
        // the paths town was opened with, it alone says
        let paths = Word::new(RESIDENTS, Tense::Present, Body::of(serde_json::json!(["/tmp"])));
        assert!(town.may(&paths, &host).is_err(), "town's to say");
        // one place, one resident: none may come where another is
        let at = Resident { at: Some(theme.place()), ..town.who("join", false) };
        assert!(
            town.come(&Resident { kinds: vec!["y".into()], ..at }).is_err(),
            "another is there"
        );
        drop(here);
        assert!(
            town.spoken_for(&theme, "x").is_err(),
            "gone with the one that speaks for it, however deep"
        );
        assert!(
            town.spoken_for(&host, "theme.lua").is_err(),
            "gone with the one that speaks for it"
        );
        let left = town.facts(&format!("{RESIDENTS}/{}", nested.place()));
        assert_eq!(left.len(), 1, "but not one it did not speak for");
        assert!(town.may(&word, &town.who("other", false)).is_ok(), "and what it owned, let go");
    }

    // a Lua resident speaks for none, as its words have no `by`: what it says under its place is
    // refused, and none comes there
    #[test]
    fn a_lua_resident_speaks_for_none() {
        let dir = scratch();
        let town = Town::with_log(dir.log());
        let lua = Resident { lua: true, kinds: vec!["x".into()], ..town.who("a", false) };
        let _here = town.come(&lua).unwrap();
        let mut errors = town.words.hear(|_, w| w.kind == ERROR, None);
        let one = Body::of(serde_json::json!({ "own": ["theme"] }));
        town.said(Word::new("town/residents/theme", Tense::Present, one), &lua);
        assert!(town.spoken_for(&lua, "theme").is_err(), "none comes");
        let e = errors.try_recv().expect("refused");
        assert!(e.body.value()["message"].as_str().unwrap().contains("speaks for none"));
        let theme = Word::new("theme", Tense::Past, Body::none());
        assert!(town.may(&theme, &town.who("other", false)).is_ok(), "nor owns");
    }

    // what the residents of an opening owned is held from the square across a reopen, until they
    // come again, or none will: a program says what it is only once it runs
    #[test]
    fn what_was_owned_is_held_from_the_square_across_a_reopen() {
        let dir = scratch();
        let town = Town::with_log(dir.log());
        let program = |town: &Town| Resident {
            file: Some("/a/themer".into()),
            own: vec!["theme".into()],
            ..town.who("themer", false)
        };
        let _first = town.come(&program(&town)).unwrap();
        town.reopen();
        let square = || Resident { own: vec!["theme".into()], ..town.who("square", true) };
        let around = || Resident { own: vec!["/theme".into()], ..town.who("square", true) };
        assert!(town.come(&square()).is_err() && town.come(&around()).is_err(), "held");
        let other = Resident { own: vec!["other".into()], ..town.who("square", true) };
        assert!(town.come(&other).is_ok(), "only what it owned");
        let again = town.come(&program(&town)).unwrap();
        assert!(town.come(&square()).is_err(), "come again, its own");
        drop(again);
        assert!(town.come(&square()).is_ok(), "and let go as it goes");
        let _third = town.come(&program(&town)).unwrap();
        town.reopen();
        town.hold_only(|_| false);
        assert!(town.come(&square()).is_ok(), "let go, as none will come");
    }

    // one said again as nothing, `{}`, is gone, and what it owned is let go
    #[test]
    fn one_said_again_as_nothing_goes() {
        let dir = scratch();
        let town = Town::with_log(dir.log());
        let host = town.who("host", false);
        let _here = town.come(&Resident { kinds: vec!["x".into()], ..host.clone() }).unwrap();
        let said = |body: serde_json::Value| {
            town.said(Word::new("town/residents/theme", Tense::Present, Body::of(body)), &host)
        };
        said(serde_json::json!({ "own": ["theme"] }));
        assert!(town.spoken_for(&host, "theme").is_ok());
        said(serde_json::json!({}));
        assert!(town.spoken_for(&host, "theme").is_err(), "gone");
        let theme = Word::new("theme", Tense::Past, Body::none());
        assert!(town.may(&theme, &town.who("other", false)).is_ok(), "and what it owned, let go");
    }

    // one place, one owner; a place under another's is another place, and its owner is
    // nearest
    #[test]
    fn the_nearest_owner_says_its_own() {
        let dir = scratch();
        let town = Town::with_log(dir.log());
        let (outer, inner, square) =
            (town.who("box", false), town.who("box/cat", false), town.who("square", true));
        let _outer = town
            .come(&Resident { own: vec!["box".into(), "/box".into()], ..outer.clone() })
            .unwrap();
        let _inner = town
            .come(&Resident { own: vec!["box/cat".into(), "/box/cat".into()], ..inner.clone() })
            .unwrap();
        let said = |kind: &str| Word::new(kind, Tense::Past, Body::none());
        assert!(town.may(&said("box/cat/a"), &inner).is_ok());
        assert!(town.may(&said("box/cat/a"), &outer).is_err(), "the nearer says it");
        assert!(town.may(&said("box/dog"), &outer).is_ok());
        assert!(
            town.come(&Resident { own: vec!["box".into()], ..square.clone() }).is_err(),
            "one owner a place"
        );
        assert!(
            town.come(&Resident { own: vec!["box/dog".into()], ..square.clone() }).is_err(),
            "nor within another's"
        );
        assert!(town.come(&Resident { own: vec!["dog".into()], ..square.clone() }).is_ok());
    }

    // one at the square owns nothing within another's, whichever came first: there first, it keeps
    // away one that would own around it
    #[test]
    fn the_square_owns_nothing_within_anothers_whoever_came_first() {
        let dir = scratch();
        let town = Town::with_log(dir.log());
        let square = Resident { own: vec!["box/cat".into()], ..town.who("square", true) };
        let _square = town.come(&square).unwrap();
        let join = |own: &str| Resident { own: vec![own.into()], ..town.who("box", false) };
        for around in ["/box", "box/"] {
            assert!(town.come(&join(around)).is_err(), "around it: {around}");
        }
        assert!(town.come(&join("box")).is_ok(), "beside it, not around it");
    }

    // who owns what follows the residents' facts: said, it owns; taken away, it lets go
    #[test]
    fn ownership_follows_a_residents_fact() {
        let dir = scratch();
        let town = Town::with_log(dir.log());
        let (a, b) = (town.who("a", false), town.who("b", false));
        let mine = Word::new("mine", Tense::Past, Body::none());
        let fact = format!("{RESIDENTS}/{}", a.id);
        let body = serde_json::json!({ "id": a.id, "own": ["mine"] });
        town.talk(Word::new(fact.as_str(), Tense::Present, Body::of(body)));
        assert!(town.may(&mine, &a).is_ok() && town.may(&mine, &b).is_err(), "said, it owns");
        assert!(town.come(&Resident { own: vec!["mine".into()], ..b.clone() }).is_err());
        town.talk(Word::new(fact.as_str(), Tense::Present, Body::none()));
        assert!(town.may(&mine, &b).is_ok(), "taken away, it lets go");
    }

    // who is here is kept in the present, never in the log; and a town started from an old log
    // that holds one has none here, and none owns what it owned
    #[test]
    fn who_is_here_is_not_logged() {
        let dir = scratch();
        let log = dir.log();
        let town = Town::with_log(log.clone());
        let who = Resident { own: vec!["mine".into()], ..town.who("a", false) };
        let _here = town.come(&who).unwrap();
        town.talk(Word::new(RESIDENTS, Tense::Present, Body::of(serde_json::json!(["/a"]))));
        town.talk(Word::new("note", Tense::Present, Body::of("kept")));
        assert_eq!(town.residents().len(), 1, "here, in the present");
        town.compact();
        let logged = std::fs::read_to_string(&log).unwrap();
        assert!(!logged.contains(RESIDENTS) && logged.contains("note"), "not in the log");
        let old = format!(
            "{{\"kind\":\"{RESIDENTS}\",\"body\":[\"/a\"]}}\n\
             {{\"kind\":\"{RESIDENTS}/9\",\"body\":{{\"id\":9,\"own\":[\"mine\"]}}}}\n"
        );
        std::fs::write(&log, old + &logged).unwrap();
        let again = Town::with_log(log.clone());
        assert!(again.facts(&format!("/{RESIDENTS}")).is_empty(), "an old log's, passed over");
        assert!(again.past(&format!("/{RESIDENTS}")).is_empty(), "nor in its past");
        assert_eq!(again.present.borrow()["note"], Body::of("kept"));
        let other = again.who("b", false);
        assert!(again.may(&Word::new("mine", Tense::Past, Body::none()), &other).is_ok());
    }

    // one coming hears all until it says what it hears, then only that of what waits; until it
    // is told, it falls behind quietly, and if more came than may wait, what waited it hears still,
    // and is said to have missed what came after
    #[test]
    fn one_coming_hears_all_until_it_says_what_it_hears() {
        let dir = scratch();
        let town = Town::with_log(dir.log());
        let who = town.who("slow", true);
        let mut errors = town.words.hear(|_, w| w.kind == "town/error", None);
        let said = |kind: &str| town.talk(Word::new(kind, Tense::Past, Body::none()));
        let mut words = town.words.coming(2, &who);
        said("a");
        said("b");
        words.hear(&town, |_, w| w.kind == "b");
        assert!(!words.told(), "as many as may wait: none let go");
        said("b");
        let heard: Vec<String> =
            std::iter::from_fn(|| words.try_recv().ok()).map(|w| w.kind).collect();
        assert_eq!(heard, ["b", "b"], "what it hears of what waited, then since");
        drop(words);
        let mut words = town.words.coming(2, &who);
        ["a", "b", "b"].into_iter().for_each(said);
        words.hear(&town, |_, w| w.kind == "b");
        ["b", "b"].into_iter().for_each(said);
        assert!(errors.try_recv().is_err(), "not said to fall behind before it is told");
        assert!(words.told(), "more than may wait: some may be missed");
        let heard: Vec<String> =
            std::iter::from_fn(|| words.try_recv().ok()).map(|w| w.kind).collect();
        assert_eq!(heard, ["b", "b"], "what waited, as many as may, it hears still");
    }

    // a resident's queue holds only so many: past that, it is said to fall behind once, however
    // far and however slowly it reads; only once it has caught up may it fall behind again
    #[test]
    fn one_behind_is_said_once_until_it_catches_up() {
        let dir = scratch();
        let town = Town::with_log(dir.log());
        let mut errors = town.words.hear(|_, w| w.kind == "town/error", None);
        let slow = town.who("slow", false);
        let mut words = town.words.hear(|_, w| w.kind == "ping", Some((2, slow)));
        let ping = || town.talk(Word::new("ping", Tense::Past, Body::none()));
        let behind = |errors: &mut Listening| std::iter::from_fn(|| errors.try_recv().ok()).count();
        (0..4).for_each(|_| ping());
        assert_eq!(behind(&mut errors), 1, "fell behind once, however far");
        for _ in 0..5 {
            words.try_recv().unwrap(); // one read, one more come, and one more dropped
            ping();
            ping();
        }
        assert_eq!(behind(&mut errors), 0, "still behind, not said again");
        while words.try_recv().is_ok() {}
        (0..3).for_each(|_| ping());
        assert_eq!(behind(&mut errors), 1, "caught up, then behind again");
    }

    // the log is compacted as it grows, so it stays bounded however long town runs, and keeps
    // the present whole
    #[test]
    fn the_log_stays_bounded() {
        let dir = scratch();
        let path = dir.log();
        let town = Town::with_log(path.clone());
        let line = "x".repeat(1000);
        for i in 0..3000 {
            town.talk(Word::new(format!("k{}", i % 50), Tense::Present, Body::of(line.as_str())));
        }
        assert!(std::fs::metadata(&path).unwrap().len() <= KEEP + GROWTH + 64 * 1024);
        assert_eq!(Town::with_log(path.clone()).present.borrow().len(), 50);
    }

    // a big present is not written again for each `GROWTH` that comes, only as the log doubles:
    // so the work of keeping it is as the facts said, not as their square
    #[test]
    fn a_big_present_is_compacted_as_the_log_doubles() {
        use std::os::unix::fs::MetadataExt;
        let dir = scratch();
        let path = dir.log();
        let town = Town::with_log(path.clone());
        let line = "x".repeat(1000);
        let (mut compacted, mut was) = (0, 0);
        for i in 0..8000 {
            town.talk(Word::new(format!("k{i}"), Tense::Present, Body::of(line.as_str())));
            // compacted, the log is a new file, renamed over the old
            let is = std::fs::metadata(&path).unwrap().ino();
            compacted += usize::from(is != was && was != 0);
            was = is;
        }
        // 8 MB said: at 1, 2, 4 and 8 MB, not at each of the eight
        assert!(compacted <= 4, "compacted {compacted} times");
        assert_eq!(Town::with_log(path.clone()).present.borrow().len(), 8000);
    }

    // compacted, the log holds each fact once, in the order it was said, and no `.tmp` is left:
    // the past is as it was, and a town rebuilt from it has the present whole, though its tail
    // holds only the last
    #[test]
    fn compaction_keeps_the_past_in_order() {
        let dir = scratch();
        let path = dir.log();
        let town = Town::with_log(path.clone());
        let say = |kind: &str, n: i64| town.talk(Word::new(kind, Tense::Present, Body::of(n)));
        say("old", 1);
        (0..3).for_each(|n| say("song", n));
        town.compact();
        assert_eq!(town.past("song")["song"], [Parts::from(0i64), 1i64.into(), 2i64.into()]);
        assert_eq!(town.past("old")["old"], [Parts::from(1i64)]);
        log::compact(&town.log, &town.present_words(), 60); // the tail holds only the last
        assert_eq!(town.past("song")["song"], [Parts::from(2i64)]);
        let rebuilt = Town::with_log(path.clone());
        assert_eq!(*rebuilt.present.borrow(), *town.present.borrow(), "the present kept");
        let mut tmp = path.clone().into_os_string();
        tmp.push(".tmp");
        assert!(!std::path::Path::new(&tmp).exists(), "no .tmp left");
    }

    // a language is a resident: spoken while it is here, and no longer once it goes, nor in a town
    // started again; none is json, none is another's, none is from the square, each a grammar
    #[test]
    fn a_language_is_a_resident() {
        let dir = scratch();
        let town = Town::with_log(dir.log());
        let line = serde_json::json!({ "upto": "\n" });
        let as_ = |name: &str, grammar: serde_json::Value, square: bool| {
            let languages = BTreeMap::from([(name.to_string(), Parts::from(grammar))]);
            town.come(&Resident { languages, ..town.who(name, square) })
        };
        let here = as_("line", line.clone(), false).unwrap();
        assert_eq!(town.languages.parts("line", b"hi\n"), Ok(Parts::from("hi")));
        let fact = town.residents().into_values().next().unwrap();
        assert_eq!(fact.value()["languages"]["line"], line, "what it is, its fact");
        let made = town.languages.get("line").unwrap();
        let _other = town.come(&Resident { kinds: vec!["x".into()], ..town.who("x", false) });
        assert!(Rc::ptr_eq(&made, &town.languages.get("line").unwrap()), "made once");
        assert!(as_("line", serde_json::json!({ "but": "\n" }), false).is_err(), "another is line");
        assert!(as_("x", serde_json::json!({ "nope": 1 }), false).is_err(), "no grammar");
        assert!(as_("json", line.clone(), false).is_err(), "json is town's own");
        assert!(as_("y", line.clone(), true).is_err(), "from the square");
        assert!(Town::with_log(dir.log()).languages.get("line").is_err(), "not in the log");
        drop(here);
        assert!(town.languages.get("line").is_err(), "gone with it");
    }

    // a line of the log that is not text is passed over, and the facts around it stand
    #[test]
    fn a_line_that_is_no_text_loses_no_fact() {
        let dir = scratch();
        let path = dir.log();
        let mut past = b"{\"kind\":\"song\",\"tense\":\"present\",\"body\":\"a\"}\n".to_vec();
        past.extend(b"{\"kind\":\"cut\",\"tense\":\"present\",\"body\":\"\xe4\xb8\n");
        past.extend(b"{\"kind\":\"theme\",\"tense\":\"present\",\"body\":\"b\"}\n");
        std::fs::write(&path, past).unwrap();
        let town = Town::with_log(path.clone());
        assert_eq!(town.present.borrow()["song"], Body::of("a"));
        assert_eq!(town.present.borrow()["theme"], Body::of("b"));
    }

    // what is no word is said back as its first `SAID` bytes, and how many more there were, so an
    // error costs little however much was said; a char is not cut
    #[test]
    fn what_is_no_word_is_said_back_cut() {
        let dir = scratch();
        let town = Town::with_log(dir.log());
        let mut errors = town.words.hear(|_, w| w.kind == ERROR, None);
        let said = format!("{}é{}", "x".repeat(SAID - 1), "y".repeat(100_000));
        town.misheard(&town.who("loud", true), &said, "not a word".into());
        let e = errors.try_recv().unwrap();
        let back = e.body.part("said").and_then(Parts::text).unwrap().to_string();
        assert_eq!(back, format!("{}… and 100002 more bytes", "x".repeat(SAID - 1)));
        assert_eq!(cut("short"), "short", "what is short, whole");
    }

    // a word in by the door is stamped by town, whatever `when` its sayer gave it
    #[test]
    fn town_stamps_each_word_said() {
        let dir = scratch();
        let town = Town::with_log(dir.log());
        let mut words = town.words.listen();
        let said = Word { when: 5, ..Word::new("x", Tense::Past, Body::none()) };
        town.said(said, &town.who("a", false));
        assert!(words.try_recv().unwrap().when > 5, "stamped as it is said");
    }

    // every word talked is heard, but only a fact folds into the present and lands in the log
    #[test]
    fn talk_folds_facts_only() {
        let dir = scratch();
        let path = dir.log();
        let town = Town::with_log(path.clone());
        let mut words = town.words.listen();
        for tense in [Tense::Present, Tense::Past, Tense::Future] {
            town.talk(Word::new("theme", tense, Body::of(tense.name())));
        }
        assert_eq!(std::iter::from_fn(|| words.try_recv().ok()).count(), 3);
        assert_eq!(*town.present.borrow(), BTreeMap::from([("theme".into(), Body::of("present"))]));
        assert_eq!(std::fs::read_to_string(&path).unwrap().lines().count(), 1);
    }
}
