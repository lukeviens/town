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
use std::rc::{Rc, Weak};

use tokio::sync::mpsc;

use crate::body::Body;
use crate::log;
use crate::parts::Parts;
use crate::word::{hears, is_kind, place, Tense, Word, ERROR, KIND};

/// A program joined: who it is, its kind, owned, and its run, among the town's tasks. Taken out of
/// `joins`, its run ends and its kind is let go.
pub struct Joined {
    pub who: Resident,
    /// the reads of that town asked through it, as yet unanswered (`Town::through`)
    pub(crate) asked: RefCell<Asked>,
    pub(crate) _here: Here,
    pub(crate) _run: crate::program::Ends,
}

/// The reads of a town asked through its join, each by the `id` town gave it there: who asked,
/// and the `id` it asked by, so its answer is told to it alone, as it asked.
#[derive(Default)]
pub struct Asked {
    next: u64,
    by: HashMap<String, (u64, Option<Parts>)>,
}

/// A resident: who says a word, as PROTOCOL.md's *Residents* says, town itself among them,
/// number 0.
///
/// While it is here, this is its fact, at its `place`: what its first word says it is, and nothing
/// else, so a misspelled field is refused, not passed over; and what town gives it.
#[derive(Clone, Default, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct Resident {
    #[serde(skip_deserializing)]
    pub id: u64,
    #[serde(skip_deserializing)]
    pub name: String,
    #[serde(default)]
    pub kinds: Vec<String>,
    /// what it does not hear, of what its kinds name
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub except: Vec<String>,
    #[serde(default)]
    pub own: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speaks: Option<crate::voice::Speaks>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub languages: BTreeMap<String, Parts>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub names: BTreeMap<String, Name>,
    /// the most words that may wait for it while it does not read; with none, all may
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub waits: Option<usize>,
    /// the most bytes of one word it is told; with none, any
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub most: Option<usize>,
    /// for each kind under a pattern it owns, how many facts of its past the log keeps; with none,
    /// all
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub keep: BTreeMap<String, u64>,
    #[serde(skip_deserializing, skip_serializing_if = "Option::is_none")]
    pub at: Option<String>,
    #[serde(skip_deserializing, skip_serializing_if = "std::ops::Not::not")]
    pub square: bool,
    #[serde(skip_deserializing, skip_serializing_if = "std::ops::Not::not")]
    pub visitor: bool,
    /// its place, if it is given one: a file's whole path, or `<its speaker's place>/<name>`
    #[serde(skip)]
    pub placed: Option<String>,
    /// the number of the resident that speaks for it, if one does
    #[serde(skip_deserializing, skip_serializing_if = "Option::is_none")]
    pub by: Option<u64>,
}

/// A name another town comes to this one by, at the square (`town '{"name":"<name>"}'`): what it
/// hears. The town it comes to says what it is, not the one that comes.
#[derive(Clone, Default, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct Name {
    #[serde(default)]
    pub kinds: Vec<String>,
    /// what it does not hear, of what its kinds name
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub except: Vec<String>,
}

/// Where town's residents are: the fact, the paths town is opened with; under it, each resident
/// here, by its `place`, as a `Resident`, and its going the fact of nothing. Who owns what is read
/// there. Who is here is so only while town runs: these facts are never logged, and one an old log
/// has is passed over (`logged`).
pub const RESIDENTS: &str = "town/residents";

/// The owner of what one away owned, held for it: a number none is given.
const AWAY: u64 = u64::MAX;

/// Whether a fact of `kind` is kept in the log: all but `RESIDENTS` and what is under it.
fn logged(kind: &str) -> bool {
    !crate::residents::under(kind, RESIDENTS)
}

/// Whether `kind` is a resident's, under `RESIDENTS`: what the owners are read from.
fn resident(kind: &str) -> bool {
    within(kind, RESIDENTS)
}

/// Whether a kind is town's own: `town` and what is under it, where town says who is here, what
/// came of what it was asked, and what failed. A resident asks of it, to work, join, open and
/// read, and says its own errors there; none owns it.
pub fn towns(kind: &str) -> bool {
    let kind = place(kind);
    kind == "town" || kind.starts_with("town/")
}

/// Every word said, to each who listens, in order: a queue each, of what it hears. A queue has no
/// bound, so none misses a word, but a resident's that says it `waits` for so many: what comes past
/// that is dropped, and said, once each time it falls behind. A queue is let go as its listener
/// ends.
#[derive(Default)]
pub struct Words(RefCell<Vec<Queue>>);

/// The queue of one who listens: what it hears, and, for a resident, who it is, to say so when
/// more are dropped than it says may wait.
struct Queue {
    to: mpsc::UnboundedSender<Told>,
    hears: Hears,
    who: Option<Resident>,
    state: Rc<State>,
}

/// Those one speaks for, each by its name from its place, as it hears: shared, as each word said
/// reads them.
pub type Spoken = Rc<[(String, Resident)]>;

/// A word in a queue, and whom it is for.
pub type Told = (Word, To);

/// Whom a word in a queue is for, as it is said: the one whose queue it is, or one it speaks for,
/// by its name.
#[derive(Clone, Debug, PartialEq)]
pub enum To {
    It,
    One(String),
}

/// Whom of those one who listens tells a word is for, as it is now, none if none hears it: a
/// resident says it after it begins to listen.
type Hears = Rc<RefCell<Box<dyn Fn(&Town, &Word) -> Vec<To>>>>;

/// What a queue and the one who reads it both know: how many words wait; whether it fell behind
/// since it last read; whether it is `told`, before which its falling behind is not said; and
/// whether more came before it was than may wait, so what came after was dropped, for `told` to
/// say.
#[derive(Default)]
struct State {
    waiting: Cell<usize>,
    /// the most that may wait, if its resident says (`waits`)
    most: Cell<Option<usize>>,
    behind: Cell<bool>,
    told: Cell<bool>,
    lost: Cell<bool>,
}

/// The words one listens to, from when it began, as many as wait.
pub struct Listening {
    from: mpsc::UnboundedReceiver<Told>,
    hears: Hears,
    state: Rc<State>,
    /// what waited as it said what it hears, and that it hears: read first
    kept: VecDeque<Told>,
}

impl Listening {
    /// Hear, from now, what `hears` hears, and of what waits, only that: as a resident says what
    /// it is.
    #[cfg(test)]
    pub fn hear(&mut self, town: &Town, hears: impl Fn(&Town, &Word) -> bool + 'static) {
        self.tell(town, alone(hears));
    }

    /// Tell, from now, each word to each `tells` says it is for, and of what waits, only that: as
    /// a resident says what it is.
    pub fn tell(&mut self, town: &Town, tells: impl Fn(&Town, &Word) -> Vec<To> + 'static) {
        let mut kept = VecDeque::new();
        while let Some(t) = self.kept.pop_front().or_else(|| self.from.try_recv().ok()) {
            match t.1 {
                To::It => kept.extend(tells(town, &t.0).into_iter().map(|to| (t.0.clone(), to))),
                To::One(_) => kept.push_back(t),
            }
        }
        self.kept = kept;
        *self.hears.borrow_mut() = Box::new(tells);
        self.state.waiting.set(self.kept.len());
    }

    /// At most `most` words wait for it, as its resident says: those after are dropped.
    pub fn waits(&mut self, most: Option<usize>) {
        self.state.most.set(most);
    }

    /// Told from now, once greeted with the present: falling behind after is said. Of what waits,
    /// each fact goes, as the present has it or one after it; what else waits stays. Whether more
    /// came than could wait, so some may be missed, for the caller to say.
    pub fn told(&mut self) -> bool {
        let lost = self.state.lost.replace(false);
        while let Ok(t) = self.from.try_recv() {
            self.kept.push_back(t);
        }
        // a fact kept is in the present it is greeted with; what else was kept it heard, though
        // more after it may be missed. A greeting of one it speaks for is not in it
        self.kept.retain(|(w, to)| *to != To::It || !w.is_fact());
        self.state.waiting.set(self.kept.len());
        self.state.told.set(true);
        lost
    }

    /// the next word, once it is said; nothing once the town is gone
    pub async fn recv(&mut self) -> Option<Word> {
        self.next().await.map(|(w, _)| w)
    }

    /// the next word, and the one spoken for it is for, if it is for one; nothing once the town
    /// is gone
    pub async fn next(&mut self) -> Option<Told> {
        let t = match self.kept.pop_front() {
            Some(t) => t,
            None => self.from.recv().await?,
        };
        Some(self.read(t))
    }

    /// the next word, if one waits
    pub fn try_recv(&mut self) -> Result<Word, mpsc::error::TryRecvError> {
        let t = match self.kept.pop_front() {
            Some(t) => t,
            None => self.from.try_recv()?,
        };
        Ok(self.read(t).0)
    }

    /// a word read: one fewer waits; and once none does, it is no longer behind
    fn read(&self, w: Told) -> Told {
        let waiting = self.state.waiting.get().saturating_sub(1);
        self.state.waiting.set(waiting);
        // caught up: what falls behind after is said again, once
        if waiting == 0 {
            self.state.behind.set(false);
        }
        w
    }
}

/// What one alone hears, as whom of those it tells a word is for: itself, or none.
fn alone(hears: impl Fn(&Town, &Word) -> bool) -> impl Fn(&Town, &Word) -> Vec<To> {
    move |town, w| if hears(town, w) { vec![To::It] } else { Vec::new() }
}

impl Queue {
    /// whether it is the queue of the resident numbered `id`
    fn of(&self, id: u64) -> bool {
        self.who.as_ref().is_some_and(|who| who.id == id)
    }
}

impl Words {
    /// listen from now: every word said after, in order, as a test does
    #[cfg(any(test, feature = "testing"))]
    pub fn listen(&self) -> Listening {
        self.hear(|_, _| true, None)
    }

    /// listen from now to what `hears` hears, in order; as `who`, which may say how many wait
    /// (`Listening::waits`), and is said to fall behind when more come
    pub fn hear(
        &self,
        hears: impl Fn(&Town, &Word) -> bool + 'static,
        who: Option<Resident>,
    ) -> Listening {
        let (to, from) = mpsc::unbounded_channel();
        let state = Rc::new(State { told: Cell::new(true), ..State::default() });
        let hears: Hears = Rc::new(RefCell::new(Box::new(alone(hears))));
        self.0.borrow_mut().push(Queue { to, hears: hears.clone(), who, state: state.clone() });
        Listening { from, hears, state, kept: Default::default() }
    }

    /// listen from now to every word, until it says what it hears (`Listening::tell`), as one comes
    /// before it says what it is; quietly, until it is told (`Listening::told`): if more come than
    /// may wait, what waits stays, and what comes after is dropped
    pub fn coming(&self, who: &Resident) -> Listening {
        // who it is, and where, to say it falls behind
        let listening = self.hear(|_, _| true, Some(who.clone()));
        listening.state.told.set(false);
        listening
    }

    /// Words for the resident numbered `id` alone, to its own queue, after what waits, before any
    /// word after: each its own, or for one it speaks for, by name. All of them, past what may
    /// wait. Not yet told, its own facts go as it is greeted with the present, which has them
    /// (`Listening::told`).
    fn greet(&self, id: u64, words: Vec<Told>) {
        let queues = self.0.borrow();
        for q in queues.iter().filter(|q| q.of(id)) {
            for w in &words {
                if q.to.send(w.clone()).is_ok() {
                    q.state.waiting.set(q.state.waiting.get() + 1);
                }
            }
        }
    }

    /// whether the resident numbered `id` has a queue of its own
    fn has_queue(&self, id: u64) -> bool {
        self.0.borrow().iter().any(|q| q.of(id) && !q.to.is_closed())
    }

    /// a word, to each who hears it; one gone is let go, and one who falls behind is said, once, as
    /// an error
    fn send(&self, town: &Town, w: Word) {
        let mut behind = Vec::new();
        self.0.borrow_mut().retain(|q| {
            if q.to.is_closed() {
                return false;
            }
            let tos = (q.hears.borrow())(town, &w);
            if tos.is_empty() {
                return true;
            }
            let state = &q.state;
            let full = state.most.get().is_some_and(|most| state.waiting.get() >= most);
            if let Some(who) = q.who.as_ref().filter(|_| full) {
                match state.told.get() {
                    true if !state.behind.replace(true) => behind.push(who.clone()),
                    true => {}
                    false => state.lost.set(true),
                }
                return true;
            }
            state.waiting.set(state.waiting.get() + tos.len());
            tos.into_iter().all(|to| q.to.send((w.clone(), to)).is_ok())
        });
        for who in behind {
            town.befell(&who, "is not reading: words dropped".into(), true);
        }
    }
}

/// A resident here: while this lives, its fact is in the present, and it owns what that says. As
/// this ends, however, the fact of nothing is said if the fact there is still its own: an ended
/// task lets go only when next run, and another may have come to its place by then. It keeps its
/// town weakly, as the town may keep it.
pub struct Here {
    town: Weak<Town>,
    kind: Option<String>,
    id: u64,
    /// kept by the one that speaks for it, which it goes with: let go here, it does not go
    kept: bool,
}

impl Here {
    /// here on, as the resident that speaks for it keeps it: it goes as that one goes
    fn stays(mut self) {
        self.kept = true;
    }
}

impl Drop for Here {
    /// it goes, and those it spoke for with it, `Town::gone`
    fn drop(&mut self) {
        let Some(town) = self.town.upgrade().filter(|_| !self.kept) else { return };
        town.went.notify_waiters();
        // what town keeps of it as it speaks, though it has no fact
        town.forget(&[self.id]);
        let Some(kind) = self.kind.take() else { return };
        let mine = town.present.borrow().get(&kind).is_some_and(|b| id(b) == Some(self.id));
        if mine {
            town.gone(&kind, self.id);
        }
    }
}

/// A number a resident's fact says: its `id`, or `by`, the one that speaks for it.
fn number(body: &Body, part: &str) -> Option<u64> {
    match body.part(part) {
        Some(Parts::Number(n)) => n.parse().ok(),
        _ => None,
    }
}

/// The languages a resident's fact says it is, each a grammar by its name.
fn spoken(body: &Body) -> &[(String, Parts)] {
    match body.part("languages") {
        Some(Parts::Map(all)) => all,
        _ => &[],
    }
}

/// What a resident's fact gives by `name` in `field`: a language it is, or a name another town
/// comes by.
fn given<'a>(body: &'a Body, field: &str, name: &str) -> Option<&'a Parts> {
    body.part(field)?.get(name)
}

/// The kinds a resident's fact says it hears.
fn kinds(body: &Body) -> impl Iterator<Item = &str> {
    texts(body, "kinds")
}

/// The texts of a list a fact says, by its name.
fn texts<'a>(body: &'a Body, part: &str) -> impl Iterator<Item = &'a str> {
    body.part(part).and_then(Parts::list).into_iter().flatten().filter_map(Parts::text)
}

/// The number of the resident a fact under `RESIDENTS` says.
fn id(body: &Body) -> Option<u64> {
    number(body, "id")
}

/// Who owns what, as a resident's fact says.
fn owners(body: &Body) -> impl Iterator<Item = Owner> + '_ {
    let (id, square) = (id(body).unwrap_or(0), body.part("square").is_some());
    let own = body.part("own").and_then(Parts::list).into_iter().flatten();
    own.filter_map(Parts::text).map(move |kind| {
        let keep = body.part("keep").and_then(|k| k.get(kind)).and_then(Parts::number);
        Owner { kind: kind.into(), id, square, keep: keep.map(|n| n as u64) }
    })
}

/// One who owns a kind, as the present says: the kind as it names it, the number of who owns it,
/// whether it came to the square, and how many facts of the past of each kind there the log
/// keeps, if it says. Read from the residents' facts each time one changes, and by nothing else.
#[derive(Clone)]
struct Owner {
    kind: String,
    id: u64,
    square: bool,
    keep: Option<u64>,
}

pub struct Town {
    /// kind → its latest present body
    pub present: RefCell<BTreeMap<String, Body>>,
    /// what tells it the moment each word is said
    clock: Box<dyn Clock>,
    /// every word, to all who listen
    pub words: Words,
    /// the fact log this town folds as it starts, and appends to: a file, or any store
    pub log: Box<dyn log::Store>,
    /// kind → how many of its facts the log keeps
    logged: RefCell<HashMap<String, u64>>,
    /// how many of those its owners keep no longer: compacted once it is as many as the rest
    over: Cell<u64>,
    /// what runs for each resident, by its number, its listening, its program or its join: ended
    /// as it goes, or the town opens again. Each keeps the town alive while it runs
    pub tasks: RefCell<Vec<(u64, tokio::task::JoinHandle<()>)>>,
    /// kind → the program joined there, ended as its asker goes, or joins there again
    pub joins: RefCell<HashMap<String, Joined>>,
    /// itself, to begin what is wanted of it as it is asked
    me: Weak<Town>,
    /// the number the next to come is given
    next: Cell<u64>,
    /// the process group of each program running, ended when the town opens again or stops
    pub groups: RefCell<HashSet<crate::unix::Pid>>,
    /// told as a resident goes, and what it held, its descriptors among them, is let go
    pub went: tokio::sync::Notify,
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
    /// those each speaker speaks for, however deep, by its number: each by its name from the
    /// speaker's place, as it says `by`, and as it hears, read from their facts as each changes
    /// (`own`)
    spoken: RefCell<HashMap<u64, Spoken>>,
    /// what residents gone to come again owned, by the kind of their fact: held against one at the
    /// square until the one that comes there again says `now`; or, for one under a speaker, until
    /// its speaker says it, or ends; or none will (`hold_only`), as a program says what it is only
    /// once it runs
    held: RefCell<Vec<(String, Owner)>>,
    /// the facts each resident has said since it came or last said `now`, by its number
    since: RefCell<HashMap<u64, HashSet<String>>>,
}

impl Resident {
    /// Whether it hears, owns, stands at and gives nothing, so has no fact.
    pub fn nothing(&self) -> bool {
        self.kinds.is_empty()
            && self.own.is_empty()
            && self.at.is_none()
            && self.languages.is_empty()
            && self.names.is_empty()
    }

    /// town itself: number 0, here from the start
    pub fn town() -> Resident {
        Resident { name: "town".into(), ..Resident::default() }
    }

    /// Where its fact is, under `RESIDENTS`: where it is placed; one at the square by its number,
    /// `square/<id>`; a join by its kind; any other by its number.
    pub fn place(&self) -> String {
        match (&self.placed, &self.at) {
            (Some(placed), _) => placed.clone(),
            _ if self.square => format!("square/{}", self.id),
            (None, Some(at)) => at.clone(),
            (None, None) => self.id.to_string(),
        }
    }
}

impl Town {
    /// A test's town, on `log`, in the process's world, by a clock that counts.
    #[cfg(any(test, feature = "testing"))]
    pub fn with_log(log: impl log::Store + 'static) -> Rc<Town> {
        Town::in_world(log, std::env::vars_os().collect())
    }

    /// A test's town, as `with_log`, that runs in `world`.
    #[cfg(any(test, feature = "testing"))]
    pub fn in_world(
        log: impl log::Store + 'static,
        world: BTreeMap<OsString, OsString>,
    ) -> Rc<Town> {
        Town::opened(log, world, Counts::default())
            .unwrap_or_else(|e| panic!("town: cannot open its log: {e}"))
    }

    /// A town on its `log`, in its `world`, by its `clock`: all it is given, nothing it takes. Or
    /// why its log cannot be read or mended: a town opens on its log, or not at all, as one it
    /// cannot read is none it may write over.
    pub fn opened(
        log: impl log::Store + 'static,
        world: BTreeMap<OsString, OsString>,
        clock: impl Clock + 'static,
    ) -> std::io::Result<Rc<Town>> {
        // the present is the fold of the past: a line that is no word is passed over
        let words = log::open(&log)?;
        let (present, logged) = fold_counted(said(words));
        let log: Box<dyn log::Store> = Box::new(log);
        Ok(Rc::new_cyclic(|me| Town {
            me: me.clone(),
            present: RefCell::new(present),
            clock: Box::new(clock),
            words: Words::default(),
            log,
            logged: RefCell::new(logged),
            over: Cell::new(0),
            tasks: RefCell::new(Vec::new()),
            joins: RefCell::new(HashMap::new()),
            next: Cell::new(0),
            groups: RefCell::new(HashSet::new()),
            went: tokio::sync::Notify::new(),
            languages: Default::default(),
            owners: Default::default(),
            opening: Default::default(),
            world,
            answering: Default::default(),
            spoken: Default::default(),
            held: Default::default(),
            since: Default::default(),
        }))
    }

    /// The moment by town's clock, as a word is stamped with.
    pub(crate) fn moment(&self) -> u64 {
        self.clock.now()
    }

    /// One come to town, by the name it goes by: given a number no other has.
    pub fn who(&self, name: &str, square: bool) -> Resident {
        self.next.set(self.next.get() + 1);
        Resident { id: self.next.get(), name: name.to_string(), square, ..Resident::default() }
    }

    /// `who` comes, as what it says it is: its fact, at its `place`, unless it hears, owns, stands
    /// at and gives nothing. It owns all it says, or none (`may_own`).
    pub fn come(self: &Rc<Self>, who: &Resident) -> Result<Here, String> {
        self.may_come(who, None)?;
        // what of the past is kept: only of what it owns
        if let Some(kind) = who.keep.keys().find(|k| !who.own.contains(k)) {
            return Err(format!("`{kind}`: it keeps only what it owns"));
        }
        let kind = format!("{RESIDENTS}/{}", who.place());
        if who.nothing() {
            return Ok(Here { town: Rc::downgrade(self), kind: None, id: who.id, kept: false });
        }
        let json = serde_json::to_vec(who).map_err(|e| e.to_string())?;
        let body = Body::of(crate::parts::json::<Parts>(&json).map_err(|e| e.to_string())?);
        // the languages it is, as its fact says them: none json, town's own, nor another's; each a
        // grammar, made as its fact is said (`own`)
        for (name, grammar) in spoken(&body) {
            if name == "json" {
                return Err("json is town's own".into());
            }
            if who.square {
                return Err(format!("`{name}`: what came to the square is no language"));
            }
            if self.another_gives(who, "languages", name) {
                return Err(format!("another is `{name}`"));
            }
            crate::language::Grammar::of(grammar).map_err(|e| format!("{name}: {e}"))?;
        }
        // the names another town may come by: each a kind, the path one that comes by it stands
        // at; none said from the square, where one would name itself; each one resident's; and
        // none that hears others' errors, which would be told there as that town's own
        for (name, named) in &who.names {
            if !crate::word::is_kind(name) {
                return Err(format!("`{name}` is no kind: {}", crate::word::KIND));
            }
            if who.square {
                return Err(format!("`{name}`: what came to the square names no town"));
            }
            if self.another_gives(who, "names", name) {
                return Err(format!("another names `{name}`"));
            }
            if crate::word::heard(&named.kinds, &named.except, ERROR) {
                return Err(format!("`{name}` hears only its own errors"));
            }
        }
        self.talk(Word::new(kind.as_str(), Tense::Present, body));
        Ok(Here { town: Rc::downgrade(self), kind: Some(kind), id: who.id, kept: false })
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
        // what is so under `town` town alone says, so none owns it
        if let Some(kind) = own.iter().find(|kind| towns(kind)) {
            return Err(format!("`{kind}` is town's own"));
        }
        // who is here is town's alone to say, so none owns it, what contains it, nor what is in it
        let residents = |kind: &str| hears(kind, RESIDENTS) || within(place(kind), RESIDENTS);
        if let Some(kind) = own.iter().find(|kind| residents(kind)) {
            return Err(format!("`{kind}` contains `{RESIDENTS}`, which is town's alone"));
        }
        let (owners, held) = (self.owners.borrow(), self.held.borrow());
        let other = |o: &&Owner| o.id != who.id && Some(o.id) != besides;
        let mut others: Vec<&Owner> = owners.iter().filter(other).collect();
        // what one away owned is held from all but it, until it comes again and says `now`
        let mine = format!("{RESIDENTS}/{}", who.place());
        others.extend(held.iter().filter(|(at, _)| *at != mine).map(|(_, o)| o));
        let taken: HashSet<&str> = others.iter().map(|o| place(&o.kind)).collect();
        if let Some(kind) = own.iter().find(|kind| taken.contains(&place(kind))) {
            // of one away, it is held for it: none else owns it yet
            let away = held.iter().any(|(at, o)| *at != mine && place(&o.kind) == place(kind));
            let live = owners.iter().filter(other).any(|o| place(&o.kind) == place(kind));
            return Err(match away && !live {
                true => format!("`{kind}` is held for one that comes again"),
                false => format!("another owns `{kind}`"),
            });
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

    /// The resident numbered `who`, as its fact says it, by its name at its place; town itself, if
    /// none is here by that number.
    pub(crate) fn resident(&self, who: u64) -> Resident {
        let fact = self.place_of(who).and_then(|place| {
            let body = self.present.borrow().get(&format!("{RESIDENTS}/{place}")).cloned()?;
            Some((place, body))
        });
        let Some((place, body)) = fact else { return Resident::town() };
        let name = body.part("name").and_then(Parts::text).unwrap_or("?").to_string();
        Resident { id: who, name, placed: Some(place), ..Resident::default() }
    }

    /// The place of the resident numbered `who`, if it is here.
    pub(crate) fn place_of(&self, who: u64) -> Option<String> {
        let mine = |(kind, body): (String, Body)| (id(&body) == Some(who)).then_some(kind);
        let kind = self.residents().into_iter().find_map(mine)?;
        kind.strip_prefix(&format!("{RESIDENTS}/")).map(String::from)
    }

    /// What one that comes by `name` is, as the resident that names it says: what it hears. Now,
    /// if one names it, or else once one does, telling `waits`, once, that it waits, as a language
    /// is waited for (`language`). Nothing if the town stops first.
    pub async fn named(&self, name: &str, waits: impl AsyncFnMut(&str)) -> Option<Name> {
        let waiting = format!("no resident names `{name}` yet: it waits for one");
        self.when_given(|| self.name(name), &waiting, waits).await
    }

    /// What `now` gives, now, or else once a resident comes or goes and it does, telling `waits`,
    /// once, `waiting`. Nothing if the town stops first.
    async fn when_given<T>(
        &self,
        now: impl Fn() -> Option<T>,
        waiting: &str,
        mut waits: impl AsyncFnMut(&str),
    ) -> Option<T> {
        let mut words = self.words.hear(|_, w| resident(&w.kind), None);
        let mut told = false;
        loop {
            if let Some(it) = now() {
                return Some(it);
            }
            if !std::mem::replace(&mut told, true) {
                waits(waiting).await;
            }
            words.recv().await?;
        }
    }

    /// Whether a resident but `who` gives `name`, among its `field`: its `languages`, or `names`.
    fn another_gives(&self, who: &Resident, field: &str, name: &str) -> bool {
        let others = self.residents().into_values().filter(|r| id(r) != Some(who.id));
        others.into_iter().any(|r| given(&r, field, name).is_some())
    }

    /// Until the name `name` is no longer `is`: it goes, or its kinds change. So one that came by
    /// it goes, to come again by it as it is now.
    pub async fn renamed(&self, name: &str, is: &Name) {
        let mut words = self.words.hear(|_, w| resident(&w.kind), None);
        while self.name(name).as_ref() == Some(is) {
            if words.recv().await.is_none() {
                return;
            }
        }
    }

    /// The name `name` as it is now: what the resident that gives it says it hears.
    fn name(&self, name: &str) -> Option<Name> {
        let residents = self.residents();
        let named = residents.values().find_map(|r| given(r, "names", name))?;
        let list = |part: &str| {
            let all = named.get(part).and_then(Parts::list).into_iter().flatten();
            all.filter_map(Parts::text).map(String::from).collect()
        };
        Some(Name { kinds: list("kinds"), except: list("except") })
    }

    /// Who is here, by place: the facts under `RESIDENTS`.
    fn residents(&self) -> BTreeMap<String, Body> {
        self.facts(&format!("{RESIDENTS}/"))
    }

    /// Those the one numbered `me` speaks for, however deep, each by its name from its place.
    pub(crate) fn spoken_by(&self, me: u64) -> Spoken {
        self.spoken.borrow().get(&me).cloned().unwrap_or_else(|| Rc::new([]))
    }

    /// Who owns what, as the present says, read again as the resident's fact at `kind` changes,
    /// before any hears it; whom each speaker speaks for (`speak`); and the languages it is, so a
    /// language is spoken from the moment it is said, and only those of the one that changed are
    /// made again.
    fn own(&self, kind: &str) {
        let residents = self.residents();
        let owned = residents.values().filter(|b| id(b).is_some());
        *self.owners.borrow_mut() = owned.flat_map(owners).collect();
        self.count_over();
        self.trim();
        self.speak(&residents, kind);
        let is = self.present.borrow().get(kind).map(|b| spoken(b).to_vec()).unwrap_or_default();
        self.languages.are(kind, &is);
    }

    /// Who speaks for whom, by number, as `facts` say it: each one's `by`.
    fn speakers(facts: &BTreeMap<String, Body>) -> HashMap<u64, u64> {
        facts.values().filter_map(|b| Some((id(b)?, number(b, "by")?))).collect()
    }

    /// Those that speak for the one numbered `at`, nearest first, however deep: the one way a
    /// speaker's chain is walked, and a loop in it ends.
    fn above(by: &HashMap<u64, u64>, mut at: u64) -> impl Iterator<Item = u64> + '_ {
        std::iter::from_fn(move || {
            at = *by.get(&at)?;
            Some(at)
        })
        .take(by.len())
    }

    /// Whom each speaker speaks for, read again from `residents`: those that speak through it,
    /// however deep, each by its name from its place, but a join, which speaks itself. The one at
    /// `kind`, said now, is greeted through its speaker, `by` it: its present, then `now`.
    fn speak(&self, residents: &BTreeMap<String, Body>, kind: &str) {
        // each one's fact, by its number, as the start of the facts of those it speaks for
        let places: HashMap<u64, String> =
            residents.iter().filter_map(|(k, b)| Some((id(b)?, format!("{k}/")))).collect();
        // who speaks for whom, by number; and the one that speaks for each: the nearest above it
        // with a voice of its own, as a join has, and one at the square
        let by = Self::speakers(residents);
        let voice = |at: u64| Self::above(&by, at).find(|&s| self.words.has_queue(s));
        let mut spoken = HashMap::<u64, Vec<(String, Resident)>>::new();
        let mut greeting = None;
        for (at, body) in residents {
            let (Some(one), Some(_)) = (id(body), number(body, "by")) else { continue };
            if body.part("at").is_some() {
                continue;
            }
            let Some(speaker) = voice(one) else { continue };
            let Some(name) = places.get(&speaker).and_then(|p| at.strip_prefix(p.as_str())) else {
                continue;
            };
            let resident = Resident {
                id: one,
                kinds: kinds(body).map(String::from).collect(),
                except: texts(body, "except").map(String::from).collect(),
                square: body.part("square").is_some(),
                ..Resident::default()
            };
            if at == kind {
                greeting = Some((speaker, name.to_string(), resident.clone()));
            }
            spoken.entry(speaker).or_default().push((name.to_string(), resident));
        }
        *self.spoken.borrow_mut() = spoken.into_iter().map(|(id, all)| (id, all.into())).collect();
        if let Some((speaker, name, one)) = greeting {
            let present = one.present(self).into_iter().chain([Word::now()]);
            self.words.greet(speaker, present.map(|w| (w, To::One(name.clone()))).collect());
        }
    }

    /// The resident at `kind`, numbered `who`, goes, and those it spoke for with it, and those they
    /// spoke for, however deep; not another under its place, as a join at `box/cat` is under
    /// one at `box`, and speaks for itself.
    pub(crate) fn gone(&self, kind: &str, who: u64) {
        let under = self.facts(&format!("{kind}/"));
        let by = Self::speakers(&under);
        let (mut going, mut gone) = (vec![kind.to_string()], vec![who]);
        for (kind, body) in &under {
            if let Some(one) = id(body).filter(|&one| Self::above(&by, one).any(|s| s == who)) {
                going.push(kind.clone());
                gone.push(one);
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
        self.forget(&gone);
        // the fact of nothing, only where a fact is: a join let go has said its own
        for kind in going.into_iter().rev() {
            if self.present.borrow().contains_key(&kind) {
                self.talk(Word::new(kind, Tense::Present, Body::none()));
            }
        }
        // ended, neither it nor those it spoke for will say `now`: what was held for them is let go
        self.held.borrow_mut().retain(|(at, _)| !crate::residents::under(at, kind));
    }

    /// What runs for the resident numbered `who`: ended as it goes (`gone`), or the town opens
    /// again.
    pub fn runs(&self, who: u64, task: tokio::task::JoinHandle<()>) {
        let mut tasks = self.tasks.borrow_mut();
        tasks.retain(|(_, t)| !t.is_finished());
        tasks.push((who, task));
    }

    /// The resident whose fact is at `kind`, numbered `who`, goes to come again, as its file is
    /// saved or its speaker says it is nothing: what it and those it spoke for owned is held from
    /// the square until they come again, or their speaker says all it speaks for is said.
    pub(crate) fn leaves(&self, kind: &str, who: u64) {
        let leaving: Vec<(String, Body)> = self
            .residents()
            .into_iter()
            .filter(|(k, b)| crate::residents::under(k, kind) && b.part("square").is_none())
            .collect();
        self.gone(kind, who);
        self.hold(&leaving);
    }

    /// `who` says `now`, as it is told it: it has said again each fact it has. Each fact it owns,
    /// as the nearest owner, that it has not said since it came or last said `now`, is nothing;
    /// what is kept under its place, of those it spoke for and did not say again, is let go; and a
    /// past word at its fact says so.
    fn now(&self, who: &Resident) {
        let said = self.since.borrow_mut().remove(&who.id).unwrap_or_default();
        let mine = format!("{RESIDENTS}/{}", who.place());
        // what it owns, what it owned before it came again among it (`nearest`): none but it said
        // there
        let kinds: Vec<String> = self.present.borrow().keys().cloned().collect();
        let mine_now = |k: &String| self.owner(k) == Some(who.id);
        let unsaid = kinds.into_iter().filter(|k| !said.contains(k) && mine_now(k));
        for kind in unsaid.collect::<Vec<_>>() {
            self.talk(Word { by: who.id, ..Word::new(kind, Tense::Present, Body::none()) });
        }
        self.held.borrow_mut().retain(|(at, _)| !crate::residents::under(at, &mine));
        // heard as an event of its fact: what it has is so
        self.talk(Word { by: who.id, ..Word::new(mine, Tense::Past, Body::none()) });
    }

    /// What residents going to come again owned, held from the square until they come (`held`).
    fn hold(&self, going: &[(String, Body)]) {
        let owned = going.iter().flat_map(|(kind, body)| owners(body).map(|o| (kind.clone(), o)));
        self.held.borrow_mut().extend(owned);
    }

    /// One `who` speaks for, at `name` under its place: what it is in `body`, checked as any
    /// resident is as it comes; or, of nothing or as nothing, `{}`, gone, and what it owned held
    /// from the square until it comes again, or `who` says all it speaks for is said, or goes. It
    /// stands as `who` stands, and goes as it goes.
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
            self.present.borrow().get(&kind).filter(|b| number(b, "by") == Some(who.id)).cloned();
        let said: Resident = match body.is_none() {
            true => Resident::default(),
            false => {
                // through JSON's bytes, which are flat, however deep what they say
                let json = serde_json::to_vec(body.parts()).map_err(|e| e.to_string())?;
                crate::parts::json(&json).map_err(|e| e.to_string())?
            }
        };
        // of nothing, or as nothing, `{}`, it has no fact: the one it had goes, to come again
        if said.nothing() {
            if let Some(gone) = was.as_ref().and_then(id) {
                self.leaves(&kind, gone);
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

    /// The resident `by` names, under the place of `who`, which speaks for it, however deep, as it
    /// is told its words by that name (`speak`): as its fact says it is, standing as `who` stands.
    pub fn spoken_for(&self, who: &Resident, by: &str) -> Result<Resident, String> {
        let none = || format!("it speaks for no resident `{by}`");
        let kind = format!("{RESIDENTS}/{}/{by}", who.place());
        let fact = self.present.borrow().get(&kind).cloned().ok_or_else(none)?;
        let id = id(&fact).ok_or_else(none)?;
        // spoken for by `who`, or by one it speaks for, however deep
        let by_of = Self::speakers(&self.residents());
        if !Self::above(&by_of, id).any(|speaker| speaker == who.id) {
            return Err(none());
        }
        let placed = Some(format!("{}/{by}", who.place()));
        // by the name its fact says, as its errors are known by it
        let last = by.rsplit('/').next().unwrap_or(by);
        let name = fact.part("name").and_then(Parts::text).unwrap_or(last).to_string();
        let one = Resident { id, name, square: who.square, placed, ..Resident::default() };
        Ok(Resident { by: number(&fact, "by"), ..one })
    }

    /// The town opens again: all that ran for it ends, residents, programs and joins, and their
    /// facts go here, at once, since an ended task lets go only when next run. What came to the
    /// square stays; what the others owned is held from it until they come again (`held`).
    pub fn reopen(&self) {
        self.tasks.borrow_mut().drain(..).for_each(|(_, h)| h.abort());
        // their programs now, not as their tasks are let go: a new run must not meet the old
        self.end_groups();
        // what all of them owned, joins too, held before any goes
        let gone: Vec<(String, Body)> = self
            .residents()
            .into_iter()
            .filter(|(_, body)| body.part("square").is_none())
            .collect();
        self.hold(&gone);
        drop(self.joins.take());
        let ids: Vec<u64> = gone.iter().filter_map(|(_, body)| id(body)).collect();
        self.forget(&ids);
        for (kind, _) in gone {
            self.talk(Word::new(kind, Tense::Present, Body::none()));
        }
    }

    /// What town keeps of the residents numbered `ids` as they speak, let go as they go: the facts
    /// each said since its last `now`, and whether it answers an error.
    fn forget(&self, ids: &[u64]) {
        self.since.borrow_mut().retain(|id, _| !ids.contains(id));
        self.answering.borrow_mut().retain(|id| !ids.contains(id));
    }

    /// What is held from the last opening, of those whose fact would be at a kind `still` keeps:
    /// the rest is let go, as none will come there.
    pub(crate) fn hold_only(&self, still: impl Fn(&str) -> bool) {
        self.held.borrow_mut().retain(|(kind, _)| still(kind));
    }

    /// The resident numbered `who`, itself or one spoken for, is told `w`: whether it is a
    /// `town/error` not its own, so what it says now may answer it (`failed`); and, with none, it
    /// is told no more.
    pub(crate) fn telling(&self, who: u64, w: Option<&Word>) {
        let mut answering = self.answering.borrow_mut();
        match w.is_some_and(|w| w.kind == ERROR && w.by != who) {
            true => answering.insert(who),
            false => answering.remove(&who),
        };
    }

    /// The resident numbered `who` is told `w` alone, into its own queue, as `Words::greet` does:
    /// not talked, so none else hears it and none keeps it. One spoken for is told through its
    /// speaker, `by` it.
    pub(crate) fn tell(&self, who: u64, w: Word) {
        let w = Word { when: self.clock.now(), ..w };
        let spoken = self.spoken.borrow();
        let mut speakers = spoken.iter();
        let by =
            speakers.find_map(|(&s, all)| all.iter().find(|(_, r)| r.id == who).map(|o| (s, o)));
        let (to, is_for) = by.map_or((who, To::It), |(s, (name, _))| (s, To::One(name.clone())));
        drop(spoken);
        self.words.greet(to, vec![(w, is_for)]);
    }

    /// End the group of every program running.
    pub fn end_groups(&self) {
        self.groups.borrow_mut().drain().for_each(crate::unix::end_group);
    }

    /// End one program's group, if it runs still: one ended already is not ended twice, as its
    /// number may be another's by then.
    pub fn end_group(&self, group: crate::unix::Pid) {
        if self.groups.borrow_mut().remove(&group) {
            crate::unix::end_group(group);
        }
    }

    /// Something failed: said as a past `town/error` word, `{ from, place, heard, message }`, so
    /// whoever listens can see it, and printed on town's stderr. `place` is where the fact of the
    /// one it is of is (`Resident::place`), so its speaker knows which of its residents it is of;
    /// none for town itself. `heard` is the kind of word in hand when it failed, if there was one.
    ///
    /// An error of a word in hand made as one answers a `town/error` is marked `answering`: it
    /// failed at one, or what it says while it hears one is refused or no word. None that hears
    /// `town/error` is told it (`Resident::hears`), as two that hear them would make each other's
    /// without end. Who answers one is who was told another's last (`telling`): the one told it,
    /// until it is told another.
    pub fn error(&self, from: &Resident, heard: Option<&str>, message: String) {
        self.failed(from, None, message, heard.map(|kind| ("heard", kind)));
    }

    /// A fact `who` said at `kind`, `town/residents/<name>`, of one it would speak for, refused:
    /// an error of `who`, its `place` the place that one would have had, `<who's place>/<name>`.
    /// Told to `who` too, into its own queue or its speaker's (`tell`), as it passes over its own
    /// errors: so it knows, and may let that one go, even while it answers an error, when the town
    /// hears none of it.
    fn refused(&self, who: &Resident, kind: &str, name: &str, message: String) {
        let place = format!("{}/{name}", who.place());
        let e = self.failed(who, Some(&place), message, Some(("heard", kind)));
        if !who.square {
            self.tell(who.id, e);
        }
    }

    /// What befell a resident as it is told, said as its error, and told to it too, into its own
    /// queue, once, past any `waits`: it hears none of its own errors else, but at the square, and
    /// there not while its queue is `full`, as when it falls behind.
    pub(crate) fn befell(&self, who: &Resident, message: String, full: bool) {
        let e = self.notice(who, message);
        if !who.square || full {
            self.tell(who.id, e);
        }
    }

    /// Something said that is no word: a `town/error`, with what was `said`, whole, so whoever said
    /// it can know it for theirs.
    pub fn misheard(&self, from: &Resident, said: &str, message: String) {
        self.failed(from, None, message, Some(("said", said)));
    }

    /// What `from` erred in, said as its error at `place`, or its own: `by` its number, so it knows
    /// its own, as does one that speaks for it (`Resident::hears`). The error, as said.
    pub(crate) fn failed(
        &self,
        from: &Resident,
        place: Option<&str>,
        message: String,
        also: Option<(&str, &str)>,
    ) -> Word {
        eprintln!("town: {}: {message}", from.name);
        let e = self.made(from, place, message, also, true);
        self.talk(e.clone());
        e
    }

    /// What befell `from`, not what it erred in, as an error: how it ended, that it waits, that it
    /// fell behind. Never an answer to an error, whatever it was told last.
    pub fn notice(&self, from: &Resident, message: String) -> Word {
        eprintln!("town: {}: {message}", from.name);
        let e = self.noted(from, message);
        self.talk(e.clone());
        e
    }

    /// The error `notice` says, made and not said: for one told it alone, as one not yet here is.
    pub(crate) fn noted(&self, from: &Resident, message: String) -> Word {
        self.made(from, None, message, None, false)
    }

    /// An error of `from`, as `failed` and `noted` make it: `erring`, of what it erred in itself.
    fn made(
        &self,
        from: &Resident,
        place: Option<&str>,
        message: String,
        also: Option<(&str, &str)>,
        erring: bool,
    ) -> Word {
        let by = from.id;
        // told another's error last, itself, or as one spoken for: whatever it errs in then, a word
        // it said or a line on its stderr, may be its answer to that error
        let answering = erring && self.answering.borrow().contains(&by);
        let mut body = Parts::map([("from", from.name.as_str()), ("message", &message)]);
        if by != 0 {
            body.set("place", place.map_or_else(|| from.place(), String::from));
        }
        if let Some((name, part)) = also {
            body.set(name, part);
        }
        // of one that failed at an error, or said what is refused, or no word, as it answered one
        if also == Some(("heard", ERROR)) || answering {
            body.set("answering", true);
        }
        Word { by, when: self.clock.now(), ..Word::new(ERROR, Tense::Past, Body::of(body)) }
    }

    /// May `who` say this word? Not town's own, from the square; and not what is, or was, of a kind
    /// another owns, as `hears` reads it. What is wanted of a kind, anyone may say: that is how one
    /// asks it. The nearest owner counts, so a join at `box/cat` says what is there, though one at
    /// `box` owns what is under it; each place has one owner, so the nearest is one.
    pub fn may(&self, w: &Word, who: &Resident) -> Result<(), String> {
        // a read only reads: what one at the square hears, it may ask town of too
        let read = w.kind == "town/read" && w.tense == Tense::Future;
        if who.square && towns(&w.kind) && !read {
            return Err(format!("`{}` is town's own", w.kind));
        }
        // what is so under `town`, and what came of what it was asked, town alone says: a resident
        // asks of it, and says its own errors
        let asks = w.tense == Tense::Future || (w.kind == ERROR && w.tense == Tense::Past);
        if towns(&w.kind) && !asks {
            return Err(format!("`{}` is town's to say: one may ask of it, not say it", w.kind));
        }
        // a want of what town does not do is refused before any hears it
        let verb = (w.kind == "town").then_some("").or_else(|| w.kind.strip_prefix("town/"));
        let verb = verb.filter(|_| w.tense == Tense::Future);
        if let Some(verb) = verb.filter(|v| !crate::work::VERBS.contains(v)) {
            return Err(match verb {
                "" => "a want of town names what it wants: `town/<verb>`".into(),
                _ => format!("town has no `{verb}`"),
            });
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
        match self.owner(&w.kind) {
            Some(AWAY) => Err("refused: held for one that comes again".into()),
            Some(owner) if owner != who.id => Err("refused: another owns it".into()),
            _ => Ok(()),
        }
    }

    /// What `says` reads of the nearest owner of `kind`, if one owns it: one place has one owner.
    /// What one away owned is still its, held until it says `now`: owned by the one at its fact, if
    /// it has come again, and by none that may say there if not.
    fn nearest<T>(&self, kind: &str, says: impl Fn(&Owner) -> Option<T>) -> Option<T> {
        let (owners, held) = (self.owners.borrow(), self.held.borrow());
        let holder = |at: &String| self.present.borrow().get(at).and_then(id).unwrap_or(AWAY);
        let held = held.iter().map(|(at, o)| Owner { id: holder(at), ..o.clone() });
        let all = owners.iter().cloned().chain(held).filter(|o| hears(&o.kind, kind));
        all.max_by_key(|o| place(&o.kind).len()).as_ref().and_then(says)
    }

    /// The number of the nearest owner of `kind`, if one owns it.
    fn owner(&self, kind: &str) -> Option<u64> {
        self.nearest(kind, |o| Some(o.id))
    }

    /// How many facts of the past of `kind` the log keeps, as its nearest owner says: with none,
    /// all.
    fn keeps(&self, kind: &str) -> Option<u64> {
        self.nearest(kind, |o| o.keep)
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
    /// stamped with the moment it is, whatever `when` it came with, and a `town/error` with its
    /// `place`; and if not, said back as a `town/error` from it.
    pub fn said(&self, w: Word, who: &Resident) {
        // a resident under its own place, placed from where it stands, as a join's words are
        // from its kind: it says that one is there, or is no longer
        let mine = format!("{RESIDENTS}/");
        if let Some(name) = w.kind.strip_prefix(&mine).filter(|_| w.tense == Tense::Present) {
            if let Err(why) = self.under(who, name, &w.body) {
                self.refused(who, &w.kind, name, why);
            }
            return;
        }
        if w.is_now() {
            return self.now(who);
        }
        // an error it says is of it: at its place, as town stamps each it says itself
        let w = match w.kind == ERROR && matches!(w.body.parts(), Parts::Map(_)) {
            true => {
                let mut body = w.body.parts().clone();
                body.set("place", who.place());
                Word { body: Body::of(body), ..w }
            }
            false => w,
        };
        // a fact that changes nothing is no word, so none may be refused it: it is said again, as
        // its sayer's `now` counts it, and goes nowhere
        let same = match w.body.is_none() {
            true => !self.present.borrow().contains_key(&w.kind),
            false => self.present.borrow().get(&w.kind) == Some(&w.body),
        };
        if w.is_fact() && same {
            self.since.borrow_mut().entry(who.id).or_default().insert(w.kind.clone());
            return;
        }
        match self.may(&w, who) {
            // a read is town's alone, as its answer is the asker's: none else is told either
            Ok(()) if w.kind == "town/read" && w.tense == Tense::Future => self.read(&w.body, who),
            Ok(()) if self.through(&w, who) => {}
            Ok(()) => {
                if w.tense == Tense::Present {
                    let mut since = self.since.borrow_mut();
                    since.entry(who.id).or_default().insert(w.kind.clone());
                }
                self.talk(Word { by: who.id, when: 0, ..w })
            }
            Err(why) => self.error(who, Some(&w.kind), why),
        }
    }

    /// A read of a town joined at `at`, asked as `at/town/read`: told to that join alone, by an
    /// `id` town gives it, as a read here is told to none but town; and its answer, said back by
    /// the join, told to the one that asked alone, by the `id` it asked by. Whether `w` was either.
    fn through(&self, w: &Word, who: &Resident) -> bool {
        let read = |at: &str| w.kind.strip_prefix(at) == Some("/town/read");
        // the answer, from the join asked
        if let Some(at) = who.at.as_deref().filter(|at| read(at) && w.tense == Tense::Past) {
            let given = w.body.part("id").and_then(Parts::text).map(String::from);
            let joins = self.joins.borrow();
            let asked =
                joins.get(at).zip(given).and_then(|(j, id)| j.asked.borrow_mut().by.remove(&id));
            drop(joins);
            // an answer none asked for, or asked for once already, is no one's
            let Some((asker, id)) = asked else {
                self.error(who, Some(&w.kind), "an answer no one here asked for".into());
                return true;
            };
            let mut body = w.body.parts().clone();
            match id {
                Some(id) => body.set("id", id),
                None => body.remove("id"),
            }
            self.tell(
                asker,
                Word { by: who.id, ..Word::new(&w.kind, Tense::Past, Body::of(body)) },
            );
            return true;
        }
        // the want, of the join there
        let Some(at) = self.joined(&w.kind).filter(|at| read(at) && w.tense == Tense::Future)
        else {
            return false;
        };
        let given = {
            let joins = self.joins.borrow();
            let Some(join) = joins.get(&at).filter(|j| j.who.id != who.id) else { return false };
            let mut asked = join.asked.borrow_mut();
            asked.next += 1;
            let given = asked.next.to_string();
            asked.by.insert(given.clone(), (who.id, w.body.part("id").cloned()));
            (join.who.id, given)
        };
        let (to, given) = given;
        let mut body = w.body.parts().clone();
        body.set("id", given.as_str());
        let w = Word { by: who.id, ..Word::new(&w.kind, Tense::Future, Body::of(body)) };
        self.tell(to, w);
        true
    }

    /// A read, answered to `who` alone, into its own queue: what town keeps of a kind. A town that
    /// comes by a name reads only what it hears, no more than its name gives; any other reads all,
    /// as the square is yours alone.
    fn read(&self, body: &Body, who: &Resident) {
        let hears = |kind: &str| {
            let heard = Word::new(kind, Tense::Past, Body::none());
            !who.visitor || who.hears(self, &heard)
        };
        match crate::work::read(self, body, hears) {
            Ok(answer) => self.tell(who.id, Word::new("town/read", Tense::Past, answer)),
            Err(why) => self.error(who, Some("town/read"), why),
        }
    }

    /// Talk a word: a fact folds into the present and is logged, and every word is sent on; but a
    /// fact that changes nothing is no word, and goes nowhere. A fact of nothing is logged too, so
    /// the present rebuilt from the log forgets as this one did. Who is here, under `RESIDENTS`, is
    /// kept in the present alone, and who owns what is read from it.
    pub fn talk(&self, mut word: Word) {
        if !is_kind(&word.kind) {
            let message = format!("`{}` is no kind: {}", word.kind, KIND);
            return self.error(&Resident::town(), None, message);
        }
        if word.when == 0 {
            word.when = self.clock.now();
        }
        // a fact said again as it is, or nothing where none is, is no word: it changes nothing
        let kept = keep(&mut self.present.borrow_mut(), &word);
        if word.is_fact() && !kept {
            return;
        }
        if kept && resident(&word.kind) {
            self.own(&word.kind);
        }
        if kept && logged(&word.kind) && log::append(&*self.log, &word) {
            let mut logged = self.logged.borrow_mut();
            let n = logged.entry(word.kind.clone()).or_insert(0);
            *n += 1;
            let over = self.keeps(&word.kind).is_some_and(|keep| *n > keep + 1);
            drop(logged);
            self.over.set(self.over.get() + u64::from(over));
            self.trim();
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
        waits: impl AsyncFnMut(&str),
    ) -> Option<Rc<crate::language::Language>> {
        let waiting = format!("no language is called {name} yet: it waits for one");
        self.when_given(|| self.languages.get(name).ok(), &waiting, waits).await
    }

    /// Compact the log once what its owners keep no longer is as much as the rest: so each time
    /// at least halves it, and its work is as the facts said, not as their square.
    fn trim(&self) {
        let all: u64 = self.logged.borrow().values().sum();
        if self.over.get() > 0 && 2 * self.over.get() >= all {
            self.compact();
        }
    }

    /// How many facts the log keeps that their owners keep no longer, read again as an owner
    /// comes, which may say what it keeps.
    fn count_over(&self) {
        let logged = self.logged.borrow();
        let over = logged.iter().map(|(kind, &n)| match self.keeps(kind) {
            Some(keep) => n.saturating_sub(keep + 1),
            None => 0,
        });
        self.over.set(over.sum());
    }

    /// Rewrite the log to what its owners keep: of each kind, its last facts, the present and as
    /// many before it as its owner keeps; of one none keeps, all. See `log::compact`.
    pub fn compact(&self) {
        match log::compact(&*self.log, |kind| self.keeps(kind)) {
            Ok(logged) => *self.logged.borrow_mut() = logged,
            Err(e) => self.error(&Resident::town(), None, format!("log left as it was: {e}")),
        }
        self.over.set(0);
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
    /// times there was none, as far as the log keeps them.
    pub fn past(&self, kind: &str) -> BTreeMap<String, Vec<Body>> {
        let mut past = BTreeMap::<String, Vec<Body>>::new();
        let said = match log::all(&*self.log) {
            Ok(words) => said(words),
            Err(e) => {
                self.error(&Resident::town(), None, format!("cannot read its log: {e}"));
                said(Vec::new())
            }
        };
        for w in said.filter(|w| w.is_fact() && !w.body.is_none() && hears(kind, &w.kind)) {
            past.entry(w.kind).or_default().push(w.body);
        }
        past
    }
}

/// What tells town the moment it stamps each word with, `when`: what the word says, never what
/// town waits on. The system's for a town that runs; one that counts for a test, so a word is
/// compared whole.
pub trait Clock {
    fn now(&self) -> u64;
}

/// The system's clock: ms since 1970.
pub struct System;

impl Clock for System {
    fn now(&self) -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    }
}

/// A clock that counts, one each time it is asked: the same each run.
#[derive(Default)]
pub struct Counts(Cell<u64>);

impl Clock for Counts {
    fn now(&self) -> u64 {
        self.0.set(self.0.get() + 1);
        self.0.get()
    }
}

/// Whether `kind` is under `at`, and not `at` itself.
fn within(kind: &str, at: &str) -> bool {
    kind.strip_prefix(at).is_some_and(|rest| rest.starts_with('/'))
}

/// The words of the log, but a fact not logged, as an old log may contain one.
fn said(words: Vec<Word>) -> impl Iterator<Item = Word> {
    words.into_iter().filter(|w| logged(&w.kind))
}

/// The present, folded from the past, and how many facts of each kind the past contains.
fn fold_counted(
    words: impl Iterator<Item = Word>,
) -> (BTreeMap<String, Body>, HashMap<String, u64>) {
    let (mut present, mut logged) = (BTreeMap::new(), HashMap::new());
    for w in words {
        *logged.entry(w.kind.clone()).or_insert(0) += 1;
        keep(&mut present, &w);
    }
    (present, logged)
}

/// The present, folded from the past: the last fact of each kind, as a town opens on it.
#[cfg(any(test, feature = "testing"))]
pub fn fold(words: impl Iterator<Item = Word>) -> BTreeMap<String, Body> {
    fold_counted(words).0
}

/// A fact sets its kind in the present. A fact of nothing, one with no body, is the fact that
/// there is none: its kind leaves the present. Whether it changed the present: a fact said again
/// as it is, or nothing where none is, does not.
fn keep(present: &mut BTreeMap<String, Body>, w: &Word) -> bool {
    if !w.is_fact() {
        return false;
    }
    match w.body.is_none() {
        true => present.remove(&w.kind).is_some(),
        // the same, as said again, leaves the present as it was, the body as it first came
        false if present.get(&w.kind) == Some(&w.body) => false,
        false => {
            present.insert(w.kind.clone(), w.body.clone());
            true
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{run, scratch};
    use serde_json::json;

    // a fact cut short as town stopped is let go as it opens again, so the next is not said after
    // it as one; and a log that cannot be read is no log, not an empty one, so none writes over it
    #[test]
    fn a_log_cut_short_loses_no_more() {
        let dir = scratch();
        let path = dir.log();
        std::fs::write(&path, "{\"kind\":\"a\",\"body\":1}\n{\"kind\":\"b\",\"tense\":\"pres")
            .unwrap();
        let town = Town::with_log(path.clone());
        town.talk(Word::new("c", Tense::Present, Body::of(3i64)));
        drop(town);
        let again = Town::with_log(path.clone());
        let kinds: Vec<String> = again.present.borrow().keys().cloned().collect();
        assert_eq!(kinds, ["a", "c"], "the one cut short let go, the next kept");
        // cut only of its line's end, a word is whole: kept, and the next said after it apart
        std::fs::write(&path, "{\"kind\":\"a\",\"body\":1}\n{\"kind\":\"b\",\"body\":2}").unwrap();
        let town = Town::with_log(path.clone());
        town.talk(Word::new("c", Tense::Present, Body::of(3i64)));
        drop(town);
        let again = Town::with_log(path.clone());
        let kinds: Vec<String> = again.present.borrow().keys().cloned().collect();
        assert_eq!(kinds, ["a", "b", "c"], "each its own");
        struct Unread;
        impl log::Store for Unread {
            fn append(&self, _: &[u8]) -> std::io::Result<()> {
                Ok(())
            }
            fn read(&self) -> std::io::Result<Vec<u8>> {
                Err(std::io::Error::other("unreadable"))
            }
            fn replace(&self, _: &[u8]) -> std::io::Result<()> {
                panic!("written over what was not read")
            }
        }
        assert!(log::compact(&Unread, |_| None).is_err(), "none written over it");
        let opened = std::panic::catch_unwind(|| Town::with_log(Unread));
        assert!(opened.is_err(), "no town opens on it");
    }

    // a read of a town joined here goes to its join alone, by an id town gives it there, and its
    // answer to the one that asked alone, by the id it asked by: as a read here is told
    #[test]
    fn a_read_of_another_town_is_the_askers_alone() {
        run(async {
            let dir = scratch();
            let town = Town::with_log(dir.log());
            let mut all = town.words.listen();
            let j = Resident { at: Some("other".into()), ..town.who("other", false) };
            let mut there = j.listening(&town);
            let here = town.come(&j).unwrap();
            let run = tokio::task::spawn_local(std::future::pending::<()>());
            let joined = Joined {
                who: j.clone(),
                asked: Default::default(),
                _here: here,
                _run: crate::program::Ends(run.abort_handle()),
            };
            town.joins.borrow_mut().insert("other".into(), joined);
            let asker = Resident { kinds: vec!["x".into()], ..town.who("asker", false) };
            let _asker = town.come(&asker).unwrap();
            let mut mine = asker.listening(&town);
            let read = |w: &Word| w.kind == "other/town/read";
            let asked = json!({ "kind": "x", "id": "q" });
            town.said(Word::new("other/town/read", Tense::Future, Body::of(asked)), &asker);
            let want = std::iter::from_fn(|| there.try_recv().ok()).find(read).expect("told it");
            let given = want.body.value()["id"].clone();
            assert_ne!(given, json!("q"), "by an id town gives it");
            let answer = json!({ "kind": "x", "id": given, "body": 5 });
            town.said(Word::new("other/town/read", Tense::Past, Body::of(answer)), &j);
            let got = std::iter::from_fn(|| mine.try_recv().ok()).find(read).expect("answered");
            assert_eq!(got.body.value(), json!({ "kind": "x", "id": "q", "body": 5 }), "as asked");
            assert!(!std::iter::from_fn(|| all.try_recv().ok()).any(|w| read(&w)), "none else");
            let joined = town.joins.borrow_mut().remove("other");
            drop(joined);
        });
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
            placed: Some("a/b/theme.lua".into()),
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
        let speaker = town.who("speaker", false);
        let here = town.come(&Resident { kinds: vec!["x".into()], ..speaker.clone() }).unwrap();
        let said = |kind: &str, body: serde_json::Value| {
            town.said(Word::new(kind, Tense::Present, Body::of(body)), &speaker)
        };
        said("town/residents/theme.lua", serde_json::json!({ "own": ["theme"] }));
        let theme = town.spoken_for(&speaker, "theme.lua").unwrap();
        assert_eq!(theme.place(), format!("{}/theme.lua", speaker.place()), "under its place");
        // one at a path is known by its name, as its fact says it
        said("town/residents/a/b/deep.lua", serde_json::json!({ "kinds": ["x"] }));
        assert_eq!(town.spoken_for(&speaker, "a/b/deep.lua").unwrap().name, "deep.lua");
        let fact = town.facts(&format!("{RESIDENTS}/{}", theme.place()));
        assert_eq!(fact.values().next().unwrap().value()["own"], serde_json::json!(["theme"]));
        let word = Word::new("theme", Tense::Present, Body::of("dark"));
        assert!(town.may(&word, &theme).is_ok(), "its own");
        assert!(town.may(&word, &speaker).is_err(), "not its speaker's");
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
        town.said(Word::new("town/residents/b", Tense::Present, mine("theme")), &speaker);
        assert!(errors.try_recv().is_ok(), "checked as any that comes: another owns theme");
        assert!(town.spoken_for(&speaker, "nobody").is_err());
        // said again, it is the same one, not another that would own what it owns
        let again = |own: &str| Word::new("town/residents/theme.lua", Tense::Present, mine(own));
        town.said(again("theme"), &speaker);
        assert!(errors.try_recv().is_err(), "the same one");
        assert_eq!(town.spoken_for(&speaker, "theme.lua").unwrap().id, theme.id);
        // one under its place that it does not speak for is not its, and stays as it goes
        let nested = Resident {
            kinds: vec!["x".into()],
            placed: Some(format!("{}/nested", speaker.place())),
            ..town.who("nested", false)
        };
        let _nested = town.come(&nested).unwrap();
        assert!(town.spoken_for(&speaker, "nested").is_err(), "it speaks for it not");
        // one it speaks for may speak for others, and they go with it, however deep
        let deeper = Word::new(
            "town/residents/x",
            Tense::Present,
            Body::of(serde_json::json!({ "kinds": ["x"] })),
        );
        town.said(deeper.clone(), &theme);
        assert!(town.spoken_for(&theme, "x").is_ok(), "spoken for by the one spoken for");
        // said as nothing, `{}`, it goes with those it spoke for; said again, they may come again
        let away = Word::new("town/residents/theme.lua", Tense::Present, Body::of(json!({})));
        town.said(away, &speaker);
        assert!(town.spoken_for(&speaker, "theme.lua").is_err(), "taken away");
        let x = town.facts(&format!("{RESIDENTS}/{}/x", theme.place()));
        assert!(x.is_empty(), "and the one it spoke for, with it");
        town.said(again("theme"), &speaker);
        let theme = town.spoken_for(&speaker, "theme.lua").unwrap();
        town.said(deeper, &theme);
        // one that says nothing of itself speaks for none
        let quiet = town.who("quiet", false);
        let _quiet = town.come(&quiet).unwrap();
        town.said(Word::new("town/residents/q", Tense::Present, mine("q")), &quiet);
        assert!(town.spoken_for(&quiet, "q").is_err(), "it has no place to speak from");
        let _ = errors.try_recv();
        // the paths town was opened with, it alone says
        let paths = Word::new(RESIDENTS, Tense::Present, Body::of(serde_json::json!(["/tmp"])));
        assert!(town.may(&paths, &speaker).is_err(), "town's to say");
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
            town.spoken_for(&speaker, "theme.lua").is_err(),
            "gone with the one that speaks for it"
        );
        let left = town.facts(&format!("{RESIDENTS}/{}", nested.place()));
        assert_eq!(left.len(), 1, "but not one it did not speak for");
        assert!(town.may(&word, &town.who("other", false)).is_ok(), "and what it owned, let go");
    }

    // a fact that changes nothing is no word, so none is refused it: one that owns nothing there,
    // saying it as it is, is heard by none, and errs in nothing
    #[test]
    fn a_fact_as_it_is_is_refused_to_none() {
        let dir = scratch();
        let town = Town::with_log(dir.log());
        let owner = Resident { own: vec!["theme".into()], ..town.who("owner", false) };
        let _owner = town.come(&owner).unwrap();
        town.said(Word::new("theme", Tense::Present, Body::of("dark")), &owner);
        let mut all = town.words.listen();
        let other = town.who("other", false);
        town.said(Word::new("theme", Tense::Present, Body::of("dark")), &other);
        assert!(all.try_recv().is_err(), "no word, and no error");
        town.said(Word::new("theme", Tense::Present, Body::of("light")), &other);
        assert_eq!(all.try_recv().unwrap().kind, ERROR, "a change is still the owner's alone");
    }

    // an error one makes as it answers an error is marked `answering`, whatever it errs in: a word
    // it said, or a line on its stderr; so two that echo errors go quiet. Not what befell it
    #[test]
    fn an_error_made_while_answering_is_marked() {
        let dir = scratch();
        let town = Town::with_log(dir.log());
        let (a, b) = (town.who("a", false), town.who("b", false));
        let theirs = Word { by: b.id, ..Word::new(ERROR, Tense::Past, Body::none()) };
        town.telling(a.id, Some(&theirs));
        let echo = town.failed(&a, None, "an echo on its stderr".into(), None);
        assert_eq!(
            echo.body.value()["answering"],
            true,
            "marked, so none that hears errors is told"
        );
        let befell = town.noted(&a, "it fell behind".into());
        assert!(befell.body.part("answering").is_none(), "what befell it is no answer");
        town.telling(a.id, None);
        let after = town.failed(&a, None, "its own".into(), None);
        assert!(after.body.part("answering").is_none(), "not once it is told another word");
    }

    // what one owned stays held, its alone, until it comes again and says `now`; then a fact it
    // said under what it owned, and has not said again, is nothing, though it owns that kind no
    // more
    #[test]
    fn one_that_owns_less_takes_its_old_facts_away() {
        let dir = scratch();
        let town = Town::with_log(dir.log());
        let p = |own: &[&str]| Resident {
            placed: Some("a/p".into()),
            own: own.iter().map(|o| o.to_string()).collect(),
            ..town.who("p", false)
        };
        let first = p(&["x", "y", "z/w"]);
        let here = town.come(&first).unwrap();
        for kind in ["x", "y"] {
            town.said(Word::new(kind, Tense::Present, Body::of(1i64)), &first);
        }
        town.leaves(&format!("{RESIDENTS}/a/p"), first.id);
        drop(here);
        let again = p(&["y"]);
        let _again = town.come(&again).unwrap();
        let square = Resident { own: vec!["x".into()], ..town.who("sq", true) };
        assert!(town.come(&square).is_err(), "held until it says now");
        let other = town.who("q", false);
        let x = Word::new("x", Tense::Present, Body::of(2i64));
        assert!(town.may(&x, &other).is_err(), "nor may another say there while it is held");
        let owning = Resident { own: vec!["x".into()], ..town.who("q", false) };
        assert!(town.come(&owning).is_err(), "nor own it, from the square or not");
        let around = Resident { own: vec!["/z".into()], ..town.who("q", false) };
        let _around = town.come(&around).unwrap();
        let held = Word::new("z/w", Tense::Present, Body::of(1i64));
        assert!(town.may(&held, &around).is_err(), "the nearer holder counts");
        town.said(Word::new("y", Tense::Present, Body::of(1i64)), &again);
        town.said(Word::now(), &again);
        assert!(!town.present.borrow().contains_key("x"), "what it owned and did not say again");
        assert!(town.present.borrow().contains_key("y"), "what it said again stays");
        assert!(town.come(&square).is_ok(), "and let go at its now");
    }

    // what the residents of an opening owned is held from the square across a reopen, until they
    // come again, or none will: a program says what it is only once it runs
    #[test]
    fn what_was_owned_is_held_from_the_square_across_a_reopen() {
        let dir = scratch();
        let town = Town::with_log(dir.log());
        let program = |town: &Town| Resident {
            placed: Some("a/themer".into()),
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

    // who is here is kept in the present, never in the log; and a town started from an old log
    // that has one has none here, and none owns what it owned
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
        let mut words = town.words.coming(&who);
        words.waits(Some(2));
        said("a");
        said("b");
        words.hear(&town, |_, w| w.kind == "b");
        assert!(!words.told(), "as many as may wait: none let go");
        said("b");
        let heard: Vec<String> =
            std::iter::from_fn(|| words.try_recv().ok()).map(|w| w.kind).collect();
        assert_eq!(heard, ["b", "b"], "what it hears of what waited, then since");
        drop(words);
        let mut words = town.words.coming(&who);
        words.waits(Some(2));
        ["a", "b", "b"].into_iter().for_each(said);
        words.hear(&town, |_, w| w.kind == "b");
        ["b", "b"].into_iter().for_each(said);
        assert!(errors.try_recv().is_err(), "not said to fall behind before it is told");
        assert!(words.told(), "more than may wait: some may be missed");
        let heard: Vec<String> =
            std::iter::from_fn(|| words.try_recv().ok()).map(|w| w.kind).collect();
        assert_eq!(heard, ["b", "b"], "what waited, as many as may, it hears still");
    }

    // one that falls behind is told so too, past its `waits`, as it hears none of its own errors
    // else
    #[test]
    fn one_behind_is_told_so() {
        let dir = scratch();
        let town = Town::with_log(dir.log());
        let slow = town.who("slow", false);
        let mut words = town.words.hear(|_, w| w.kind == "ping", Some(slow));
        words.waits(Some(1));
        (0..3).for_each(|_| town.talk(Word::new("ping", Tense::Past, Body::none())));
        let told: Vec<Word> = std::iter::from_fn(|| words.try_recv().ok()).collect();
        let dropped = told.iter().find(|w| w.kind == ERROR).expect("told it fell behind");
        assert_eq!(dropped.body.value()["message"], "is not reading: words dropped");
    }

    // a resident's queue keeps only so many: past that, it is said to fall behind once, however
    // far and however slowly it reads; only once it has caught up may it fall behind again
    #[test]
    fn one_behind_is_said_once_until_it_catches_up() {
        let dir = scratch();
        let town = Town::with_log(dir.log());
        let mut errors = town.words.hear(|_, w| w.kind == "town/error", None);
        let slow = town.who("slow", false);
        let mut words = town.words.hear(|_, w| w.kind == "ping", Some(slow));
        words.waits(Some(2));
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

    /// one history kept in `store`: facts said, one let go, the log compacted, and a town opened
    /// again from it. What that town has, and the log's words, whole: a test's town stamps by a
    /// count
    fn kept(store: impl log::Store + Clone + 'static) -> (Vec<(String, Body)>, Vec<String>) {
        let mut out = (Vec::new(), Vec::new());
        run(async {
            let town = Town::with_log(store.clone());
            let fact = |kind: &str, body: Body| Word::new(kind, Tense::Present, body);
            for w in
                [fact("a", Body::of(1i64)), fact("b", Body::of(1i64)), fact("a", Body::of(2i64))]
            {
                town.talk(w);
            }
            for n in 0..3i64 {
                town.talk(fact("song", Body::of(n)));
            }
            town.talk(fact("b", Body::none()));
            log::compact(&*town.log, |k| (k == "song").then_some(0)).unwrap();
            let again = Town::with_log(store);
            out.0 = again.present.borrow().clone().into_iter().collect();
            let words = log::all(&*again.log).unwrap().into_iter();
            out.1 = words.map(|w| crate::voice::line(&w)).collect();
        });
        out
    }

    // a log asks nothing of where it is kept but lines in order: kept in a file or in memory, one
    // history is the same present, opened again, and the same words
    #[test]
    fn a_log_is_kept_in_any_store() {
        let file = kept(scratch().log());
        let memory = kept(Rc::new(log::Memory::default()));
        let kinds: Vec<&str> = file.0.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(kinds, ["a", "song"], "b let go: {:?}", file.0);
        assert_eq!(file.0[0].1, Body::of(2i64));
        assert_eq!(file.0[1].1, Body::of(2i64), "the last song, kept");
        let songs = file.1.iter().filter(|l| l.contains("\"song\"")).count();
        assert_eq!(songs, 1, "compacted, in either: {:?}", file.1);
        assert_eq!(file, memory, "the same, from either store");
    }

    /// the log's facts of `kind`
    fn logged_of(log: &dyn log::Store, kind: &str) -> usize {
        log::all(log).unwrap().into_iter().filter(|w| w.kind == kind).count()
    }

    // the log keeps all, but what an owner says it keeps: of each kind it owns, the present and as
    // many facts before it; so the log contains what is kept, and as much again at most
    #[test]
    fn the_log_keeps_what_an_owner_keeps() {
        let dir = scratch();
        let path = dir.log();
        let town = Town::with_log(path.clone());
        let say = |kind: &str, n: i64| town.talk(Word::new(kind, Tense::Present, Body::of(n)));
        (0..10).for_each(|n| say("all", n));
        (0..10).for_each(|n| say("song", n));
        let owner = Resident { own: vec!["song".into()], ..town.who("songs", false) };
        let owner = Resident { keep: BTreeMap::from([("song".into(), 2)]), ..owner };
        let _here = town.come(&owner).unwrap();
        // past many compactions, each as what it no longer keeps is as much as the rest
        for n in 10..100 {
            say("song", n);
            assert!(logged_of(&path, "song") <= 2 * logged_of(&path, "all") + 6);
        }
        assert_eq!(logged_of(&path, "all"), 10, "what none keeps less of, all");
        town.compact();
        let kept: Vec<Parts> = (97..100i64).map(Parts::from).collect();
        assert_eq!(town.past("song")["song"], kept, "the present, and two before it");
        assert_eq!(Town::with_log(path.clone()).present.borrow()["song"], Body::of(99i64));
    }

    // a log is written again only as what its owners no longer keep is as much as the rest: so the
    // work of keeping it is as the facts said, not as their square
    #[test]
    fn a_log_is_compacted_as_it_halves() {
        use std::os::unix::fs::MetadataExt;
        let dir = scratch();
        let path = dir.log();
        let town = Town::with_log(path.clone());
        (0..1000i64)
            .for_each(|n| town.talk(Word::new(format!("k{n}"), Tense::Present, Body::of(n))));
        let owner = Resident { own: vec!["song".into()], ..town.who("songs", false) };
        let owner = Resident { keep: BTreeMap::from([("song".into(), 0)]), ..owner };
        let _here = town.come(&owner).unwrap();
        let (mut compacted, mut was) = (0, std::fs::metadata(&path).unwrap().ino());
        for n in 0..4000i64 {
            town.talk(Word::new("song", Tense::Present, Body::of(n)));
            // compacted, the log is a new file, renamed over the old
            let is = std::fs::metadata(&path).unwrap().ino();
            compacted += usize::from(is != was);
            was = is;
        }
        assert!(compacted <= 4, "compacted {compacted} times");
        assert_eq!(Town::with_log(path.clone()).present.borrow().len(), 1001);
    }

    // compacted, the log contains each fact once, in the order it was said, and no `.tmp` is left:
    // the past is as it was, and a town rebuilt from it has the present whole
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
        log::compact(&*town.log, |k| (k == "song").then_some(0)).unwrap();
        assert_eq!(town.past("song")["song"], [Parts::from(2i64)]);
        assert_eq!(town.past("old")["old"], [Parts::from(1i64)]);
        let rebuilt = Town::with_log(path.clone());
        assert_eq!(*rebuilt.present.borrow(), *town.present.borrow(), "the present kept");
        let mut tmp = path.clone().into_os_string();
        tmp.push(".tmp");
        assert!(!std::path::Path::new(&tmp).exists(), "no .tmp left");
    }

    // what keeps the past of a kind owns it: one that keeps what it does not own is refused
    #[test]
    fn one_keeps_only_what_it_owns() {
        let dir = scratch();
        let town = Town::with_log(dir.log());
        let keeps = Resident { keep: BTreeMap::from([("song".into(), 2)]), ..town.who("k", false) };
        assert!(town.come(&keeps).is_err());
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

    // what is no word is said back whole, so whoever said it knows what it said
    #[test]
    fn what_is_no_word_is_said_back_whole() {
        let dir = scratch();
        let town = Town::with_log(dir.log());
        let mut errors = town.words.hear(|_, w| w.kind == ERROR, None);
        let said = format!("{}é", "x".repeat(100_000));
        town.misheard(&town.who("loud", true), &said, "not a word".into());
        let e = errors.try_recv().unwrap();
        assert_eq!(e.body.part("said").and_then(Parts::text), Some(&*said));
    }

    // a word in by the door is stamped by town, whatever `when` its sayer gave it
    #[test]
    fn town_stamps_each_word_said() {
        let dir = scratch();
        let town = Town::with_log(dir.log());
        let mut words = town.words.listen();
        let said = Word { when: 5, ..Word::new("x", Tense::Past, Body::none()) };
        town.said(said, &town.who("a", false));
        assert_eq!(words.try_recv().unwrap().when, 1, "stamped by town's clock, not as it came");
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

    // what is said by one spoken for is its speaker's: so a speaker at the square told an error of
    // it is told the next too; and one inside hears it as any error, by its place, known still
    // once its fact is gone
    #[test]
    fn an_error_of_one_spoken_for_is_its_speakers() {
        let dir = scratch();
        let town = Town::with_log(dir.log());
        let square = Resident { kinds: vec!["x".into()], ..town.who("sq", true) };
        let _here = town.come(&square).unwrap();
        let _voice = square.listening(&town);
        let it = Body::of(serde_json::json!({ "kinds": ["x"] }));
        town.said(Word::new("town/residents/a", Tense::Present, it), &square);
        let a = town.spoken_for(&square, "a").unwrap();
        let mut errors = town.words.hear(|_, w| w.kind == ERROR, None);
        let work = || Word::new("town/work", Tense::Future, Body::none());
        town.said(work(), &a);
        let first = errors.try_recv().expect("refused");
        let to = square.tells()(&town, &first);
        assert_eq!(to, [To::One("a".into())], "its own, at the square, told `by` it alone");
        town.telling(square.id, Some(&first));
        town.said(work(), &a);
        assert!(errors.try_recv().is_ok(), "the next refused is said too");
        // one gone: its error is its speaker's still
        let inside = Resident { kinds: vec![ERROR.into()], ..town.who("in", false) };
        let _in = town.come(&inside).unwrap();
        let join = Resident { by: Some(inside.id), at: Some("j".into()), ..town.who("j", false) };
        drop(town.come(&join).unwrap());
        town.error(&join, None, "cannot join".into());
        let e = errors.try_recv().expect("said");
        assert!(inside.hears(&town, &e), "its join's, though the join is gone");
        assert_eq!(e.body.value()["place"], "j", "by its place");
    }

    // a speaker hears the errors of those it speaks for, each with its place, a sibling's to one
    // that hears errors among them; and never its own
    #[test]
    fn a_speaker_hears_the_errors_of_those_it_speaks_for_not_its_own() {
        let dir = scratch();
        let town = Town::with_log(dir.log());
        let owner = Resident { own: vec!["theme".into()], ..town.who("owner", false) };
        let _owner = town.come(&owner).unwrap();
        let s = Resident { kinds: vec!["s".into()], ..town.who("s", false) };
        let _s = town.come(&s).unwrap();
        let _voice = s.listening(&town);
        let it = |kinds: serde_json::Value| Body::of(serde_json::json!({ "kinds": kinds }));
        town.said(Word::new(format!("{RESIDENTS}/a"), Tense::Present, it(json!([ERROR]))), &s);
        town.said(Word::new(format!("{RESIDENTS}/b"), Tense::Present, it(json!(["x"]))), &s);
        let b = town.spoken_for(&s, "b").unwrap();
        let mut errors = town.words.hear(|_, w| w.kind == ERROR, None);
        let theme = || Word::new("theme", Tense::Past, Body::none());
        town.said(theme(), &b);
        let e = errors.try_recv().expect("refused");
        assert_eq!(
            s.tells()(&town, &e),
            [To::One("a".into())],
            "a sibling's, `by` a, as a hears errors"
        );
        assert_eq!(e.body.value()["place"], b.place(), "and whose it is");
        let says = Body::of(json!({ "message": "mine", "place": "elsewhere" }));
        town.said(Word::new(ERROR, Tense::Past, says), &b);
        let e = errors.try_recv().expect("said");
        assert_eq!(e.body.value()["place"], b.place(), "one it says, at its place");
        town.said(theme(), &s);
        let e = errors.try_recv().expect("refused");
        assert_eq!(e.body.value()["place"], s.place());
        let to = s.tells()(&town, &e);
        assert_eq!(to, [To::One("a".into())], "its own, never; though `by` a, as a hears errors");
    }

    // a name another town comes by is one resident's: none gives it from the square, where it
    // would name itself, and a second that gives it is refused; once given, it says what one that
    // comes by it hears, never others' errors
    #[test]
    fn a_name_is_one_residents() {
        run(async {
            let dir = scratch();
            let town = Town::with_log(dir.log());
            let names = || {
                BTreeMap::from([(
                    "luke".into(),
                    Name { kinds: vec!["theme".into()], ..Name::default() },
                )])
            };
            let giving = |who: Resident| Resident { names: names(), ..who };
            assert!(town.come(&giving(town.who("sq", true))).is_err(), "not from the square");
            let _gives = town.come(&giving(town.who("a", false))).unwrap();
            let again = town.come(&giving(town.who("b", false))).err();
            assert_eq!(again.as_deref(), Some("another names `luke`"), "one resident's");
            let named = town.named("luke", async |_: &str| {}).await.unwrap();
            assert_eq!(named.kinds, ["theme"], "what one that comes by it hears");
            let errors = BTreeMap::from([(
                "x".into(),
                Name { kinds: vec![ERROR.into()], ..Name::default() },
            )]);
            let hears_errors = Resident { names: errors, ..town.who("c", false) };
            let refused = town.come(&hears_errors).err();
            assert_eq!(refused.as_deref(), Some("`x` hears only its own errors"));
            // a name is a path, so a kind: one that is not would stand nowhere it can be found
            for name in ["luke/", "/luke", "a//b"] {
                let named = BTreeMap::from([(name.to_string(), Name::default())]);
                let refused = town.come(&Resident { names: named, ..town.who("d", false) }).err();
                assert!(refused.is_some_and(|e| e.contains("is no kind")), "{name}");
            }
        });
    }

    // what town keeps of one as it speaks goes as it goes, though it has no fact, as one that
    // only talks has none
    #[test]
    fn what_town_keeps_of_one_goes_as_it_goes() {
        let dir = scratch();
        let town = Town::with_log(dir.log());
        let talker = town.who("talker", true);
        let here = town.come(&talker).unwrap();
        town.said(Word::new("theme", Tense::Present, Body::of(json!({ "bg": "dark" }))), &talker);
        assert!(town.since.borrow().contains_key(&talker.id), "what it said is kept");
        drop(here);
        assert!(!town.since.borrow().contains_key(&talker.id), "and let go as it goes");
    }

    // what one takes out is true of the present as of each word: one greeted is told what its kinds
    // hear, but what it takes out
    #[test]
    fn what_one_takes_out_it_is_not_greeted_with() {
        let dir = scratch();
        let town = Town::with_log(dir.log());
        let say = |kind: &str| town.talk(Word::new(kind, Tense::Present, Body::of(1i64)));
        say("x/a");
        say("y");
        let (kinds, except) = (vec!["/".into()], vec!["x/".into()]);
        let who = Resident { kinds, except, ..town.who("mirror", false) };
        let greeted: Vec<String> = who.present(&town).into_iter().map(|w| w.kind).collect();
        assert!(greeted.contains(&"y".into()) && !greeted.contains(&"x/a".into()), "{greeted:?}");
    }

    // who is here is town's alone to say: none owns it, nor what contains it, so none's `now` takes
    // it away
    #[test]
    fn none_owns_who_is_here() {
        let dir = scratch();
        let town = Town::with_log(dir.log());
        for own in [RESIDENTS, "/town", "town/", "/town/residents"] {
            let who = Resident { own: vec![own.into()], ..town.who("greedy", false) };
            assert!(town.come(&who).is_err(), "`{own}`");
        }
        let mine = Resident { own: vec!["town/mine".into()], ..town.who("mine", false) };
        assert!(town.come(&mine).is_err(), "nor what else is town's own");
    }

    // one spoken for speaks for others as any does: its speaker names them from its own place,
    // however deep, and speaks by those names; and a fact of theirs refused is told to its speaker,
    // as one spoken for has no queue of its own
    #[test]
    fn a_speaker_speaks_by_the_names_of_those_deep_under_it() {
        let dir = scratch();
        let town = Town::with_log(dir.log());
        let owner = Resident { own: vec!["theme".into()], ..town.who("owner", false) };
        let _owner = town.come(&owner).unwrap();
        let s = Resident { kinds: vec!["s".into()], ..town.who("s", false) };
        let _s = town.come(&s).unwrap();
        let mut mine = s.listening(&town);
        let says = |kind: &str, body: serde_json::Value, by: &Resident| {
            town.said(Word::new(format!("{RESIDENTS}/{kind}"), Tense::Present, Body::of(body)), by)
        };
        says("a", json!({ "kinds": ["x"] }), &s);
        let a = town.spoken_for(&s, "a").unwrap();
        says("b", json!({ "kinds": ["x"] }), &a);
        let b = town.spoken_for(&s, "a/b").expect("by its name from the speaker's place");
        assert_eq!(b.place(), format!("{}/a/b", s.place()));
        assert!(town.spoken_for(&owner, "a/b").is_err(), "not another's");
        says("c", json!({ "own": ["theme"] }), &a);
        let told = std::iter::from_fn(|| mine.try_recv().ok()).find(|w| w.kind == ERROR);
        let place = told.map(|w| w.body.value()["place"].clone());
        assert_eq!(place, Some(json!(format!("{}/a/c", s.place()))), "told to its speaker");
    }

    // a fact a speaker says of one it would speak for, refused, is its error, at the place that
    // one would have had, and told to it, though it hears nothing of errors; while it answers an
    // error, it is said marked `answering`, which none that hears errors is told, and told to it
    // still
    #[test]
    fn a_refused_fact_is_the_speakers_error_at_the_place_it_would_have() {
        let dir = scratch();
        let town = Town::with_log(dir.log());
        let owner = Resident { own: vec!["theme".into()], ..town.who("owner", false) };
        let _owner = town.come(&owner).unwrap();
        let s = Resident { kinds: vec!["s".into()], ..town.who("s", false) };
        let _s = town.come(&s).unwrap();
        let mut mine = s.listening(&town);
        let mut errors = town.words.hear(|_, w| w.kind == ERROR, None);
        let theme = Body::of(json!({ "own": ["theme"] }));
        town.said(Word::new(format!("{RESIDENTS}/x"), Tense::Present, theme), &s);
        let e = errors.try_recv().expect("refused");
        let body = e.body.value();
        assert_eq!((body["from"].clone(), e.by), (json!("s"), s.id), "the speaker's");
        assert_eq!(body["heard"], format!("{RESIDENTS}/x"), "as it said it");
        assert_eq!(body["place"], format!("{}/x", s.place()), "where that one would be");
        assert!(body["message"].as_str().unwrap().contains("another owns"));
        let told = std::iter::from_fn(|| mine.try_recv().ok()).find(|w| w.kind == ERROR);
        assert_eq!(told.map(|w| w.body), Some(e.body.clone()), "told to it");
        town.telling(s.id, Some(&Word { by: owner.id, ..e }));
        let theme = Body::of(json!({ "own": ["theme"] }));
        town.said(Word::new(format!("{RESIDENTS}/y"), Tense::Present, theme), &s);
        let answer = errors.try_recv().expect("an answer to an error is said");
        assert_eq!(answer.body.value()["answering"], true, "marked, so none that hears errors is");
        let other = Resident { kinds: vec![ERROR.into()], ..town.who("other", false) };
        assert!(!other.hears(&town, &answer), "none that hears errors is told it");
        let told = std::iter::from_fn(|| mine.try_recv().ok()).find(|w| w.kind == ERROR);
        let place = told.map(|w| w.body.value()["place"].clone());
        assert_eq!(place, Some(json!(format!("{}/y", s.place()))), "it is told still");
    }

    // one that says `now` has said again each fact it has: what it owns, nearest, and has not said
    // since it came or last said `now` is nothing; what another owns nearer, or none owns, stays
    #[test]
    fn now_lets_go_what_is_not_said_again() {
        let dir = scratch();
        let town = Town::with_log(dir.log());
        for kind in ["box/a", "box/b", "box/cat", "free"] {
            town.talk(Word::new(kind, Tense::Present, Body::of(kind)));
        }
        let owns = |own: &[&str], name: &str| Resident {
            own: own.iter().map(|o| o.to_string()).collect(),
            ..town.who(name, false)
        };
        let (b, cat) = (owns(&["box", "/box"], "box"), owns(&["box/cat"], "cat"));
        let (_b, _cat) = (town.come(&b).unwrap(), town.come(&cat).unwrap());
        let kinds = || town.facts("/").into_keys().filter(|k| !resident(k)).collect::<Vec<_>>();
        town.said(Word::new("box/a", Tense::Present, Body::of("again")), &b);
        town.said(Word::now(), &b);
        assert_eq!(kinds(), ["box/a", "box/cat", "free"], "what it said again, and none else");
        town.said(Word::now(), &b);
        assert_eq!(kinds(), ["box/cat", "free"], "said before its last `now`, not since");
    }

    // what one a speaker speaks for owned is held from the square from its fact of nothing until
    // it comes again, or its speaker says all it speaks for is said, `now`, or ends; what is not
    // said again is let go then
    #[test]
    fn what_one_spoken_for_owned_is_held_until_all_is_said() {
        let dir = scratch();
        let town = Town::with_log(dir.log());
        let s = Resident { kinds: vec!["s".into()], ..town.who("s", false) };
        let here = town.come(&s).unwrap();
        let sub = |own: serde_json::Value| {
            let body = if own.is_null() { Body::none() } else { Body::of(json!({ "own": own })) };
            town.said(Word::new(format!("{RESIDENTS}/f.lua"), Tense::Present, body), &s);
        };
        let all = Word::now();
        let square = || Resident { own: vec!["theme".into()], ..town.who("square", true) };
        sub(json!(["theme"]));
        sub(json!(null));
        assert!(town.come(&square()).is_err(), "held between its fact of nothing and its new one");
        sub(json!(["theme"]));
        assert!(town.come(&square()).is_err(), "its own again");
        sub(json!(null));
        town.said(all.clone(), &s);
        assert!(town.come(&square()).is_ok(), "not said again: let go, as all is said");
        sub(json!(["other"]));
        sub(json!(null));
        let other = || Resident { own: vec!["other".into()], ..town.who("square", true) };
        assert!(town.come(&other()).is_err(), "held");
        drop(here);
        assert!(town.come(&other()).is_ok(), "let go as its speaker ends");
    }

    // what is held under a speaker outlives a whole reopen, as it comes again, until it says all it
    // speaks for is said
    #[test]
    fn what_is_held_under_a_speaker_outlives_a_reopen_until_all_is_said() {
        let dir = scratch();
        let town = Town::with_log(dir.log());
        let speaker = |town: &Town| Resident {
            placed: Some("a/lua".into()),
            kinds: vec!["s".into()],
            ..town.who("lua", false)
        };
        let s = speaker(&town);
        let _first = town.come(&s).unwrap();
        let theme = Body::of(json!({ "own": ["theme"] }));
        town.said(Word::new(format!("{RESIDENTS}/f.lua"), Tense::Present, theme), &s);
        town.reopen();
        let square = || Resident { own: vec!["theme".into()], ..town.who("square", true) };
        let s = speaker(&town);
        let _again = town.come(&s).unwrap();
        assert!(town.come(&square()).is_err(), "held as it comes again");
        town.said(Word::now(), &s);
        assert!(town.come(&square()).is_ok(), "let go as all is said");
    }

    // a speaker hears what those it speaks for hear, however deep, as their facts say: not what is
    // wanted of them, and not once they go, unless it or another hears it still
    #[test]
    fn a_speaker_hears_what_those_it_speaks_for_hear() {
        crate::testing::run(async {
            let dir = scratch();
            let town = Town::with_log(dir.log());
            let s = Resident { kinds: vec!["s".into()], ..town.who("s", false) };
            let _s = town.come(&s).unwrap();
            let word = |kind: &str| Word::new(kind, Tense::Past, Body::none());
            let _voice = s.listening(&town);
            let light = |town: &Town, w: &Word| !s.tells()(town, w).is_empty();
            let hears = |kind: &str| light(&town, &word(kind));
            let it = |kinds: serde_json::Value| Body::of(serde_json::json!({ "kinds": kinds }));
            let say = |name: &str, body: Body, by: &Resident| {
                town.said(Word::new(format!("{RESIDENTS}/{name}"), Tense::Present, body), by)
            };
            assert!(!hears("x"), "none it speaks for hears it yet");
            say("a", it(serde_json::json!(["x"])), &s);
            assert!(hears("x") && hears("s"), "one it speaks for hears it, and its own");
            let a = town.spoken_for(&s, "a").unwrap();
            say("b", it(serde_json::json!(["y/"])), &a);
            assert!(hears("y/1"), "one that one speaks for hears it: two deep");
            // one it speaks for stands at a join: what is wanted there is told to it, not to s
            let j = Resident { by: Some(s.id), at: Some("j".into()), ..town.who("j", false) };
            let here = town.come(&j).unwrap();
            let run = tokio::task::spawn_local(std::future::pending::<()>());
            let joined = Joined {
                who: j.clone(),
                asked: Default::default(),
                _here: here,
                _run: crate::program::Ends(run.abort_handle()),
            };
            town.joins.borrow_mut().insert("j".into(), joined);
            let wanted = Word::new("j", Tense::Future, Body::none());
            assert!(j.hears(&town, &wanted) && !light(&town, &wanted), "not what is wanted of it");
            let joined = town.joins.borrow_mut().remove("j");
            drop(joined);
            // one goes: what it heard is heard no more, but what another hears, or s itself
            say("c", it(serde_json::json!(["x", "s"])), &s);
            say("a", Body::none(), &s);
            assert!(!hears("y/1"), "gone, and those it spoke for with it");
            assert!(hears("x") && hears("s"), "another hears it still, and it does");
            say("c", Body::none(), &s);
            assert!(!hears("x") && hears("s"), "none it speaks for hears it");
        });
    }

    // one a speaker speaks for is greeted with its own present, `by` it, though the speaker hears
    // the same fact for itself
    #[test]
    fn one_spoken_for_is_greeted_with_its_own_present() {
        let dir = scratch();
        let town = Town::with_log(dir.log());
        town.talk(Word::new(RESIDENTS, Tense::Present, Body::of(serde_json::json!(["/a"]))));
        let lua = Resident { kinds: vec![format!("/{RESIDENTS}")], ..town.who("lua", false) };
        let _here = town.come(&lua).unwrap();
        let mut words = lua.listening(&town);
        let reload = Body::of(serde_json::json!({ "kinds": [RESIDENTS, "/reload"] }));
        town.said(Word::new(format!("{RESIDENTS}/reload.lua"), Tense::Present, reload), &lua);
        let mut greeted = Vec::new();
        while let Ok((w, to)) = words.from.try_recv() {
            if matches!(to, To::One(_)) {
                greeted.push((w.kind, to == To::One("reload.lua".into())));
            }
        }
        assert_eq!(greeted, [(RESIDENTS.to_string(), true), (String::new(), true)], "then `now`");
    }
}
