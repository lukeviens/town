//! A voice: how a resident speaks, a program, one at the square, a join, a socket. The README's
//! *residents*.
//!
//! - **first**: it says what it is, a `Resident`, and town says that as its fact.
//! - **where it stands**: the root, or, joined, at a kind. What it says is read here as words, each
//!   by its kind's path from there.
//! - **saying**: in by the one door, `Town::said`.
//! - **told**: by `tell`.
//! - **its language**: JSON, a line each, unless it speaks another; a grammar, as far as it reads.
//!
//! Here alone bytes become words, and words bytes.

use std::borrow::Cow;
use std::rc::Rc;

use serde::Deserialize;
use serde_json::value::RawValue;
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};

use crate::body::Body;
use crate::language::{Language, Languages, Read};
use crate::parts::Parts;
use crate::town::{Here, Listening, Resident, Town};
use crate::word::{hears, is_kind, Tense, Word, KIND};

/// The language a resident speaks: one, both ways, or the one it says and the one it hears.
#[derive(Deserialize, serde::Serialize, Clone)]
#[serde(untagged)]
pub enum Speaks {
    One(String),
    Two([String; 2]),
}

impl Speaks {
    /// parts, as the languages they name: one, or two, `[says, hears]`
    pub fn of(p: &Parts) -> Result<Speaks, String> {
        let names = "`speaks` names a language, or two: `[says, hears]`";
        serde_json::to_value(p).and_then(Speaks::deserialize).map_err(|_| names.into())
    }

    /// the language it says, and the one it hears, by name
    pub fn names(&self) -> (&str, &str) {
        match self {
            Speaks::One(one) => (one, one),
            Speaks::Two([says, hears]) => (says, hears),
        }
    }
}

impl Resident {
    /// the language it says, and the one it hears, by name: JSON if it names none
    pub fn languages(&self) -> (&str, &str) {
        self.speaks.as_ref().map_or(("json", "json"), Speaks::names)
    }

    /// Whether it is told this word: of its kinds, its own among them, so each hears in town's one
    /// order; and, joined, each future word at its kind or under it, unless a nearer join
    /// stands there. Never its own errors, those of the ones it speaks for among them
    /// (`Town::failed`), each of which could make another; but one at the square hears its own
    /// always, whatever it names: it has no other way to be told.
    pub fn hears(&self, town: &Town, w: &Word) -> bool {
        // its own error, by who it is, not by name, which many may go by
        if w.kind == crate::word::ERROR && w.by == self.id && self.id != 0 {
            return self.square;
        }
        self.wanted(town, w) || self.kinds.iter().any(|k| hears(k, &w.kind))
    }

    /// what it hears, as `hears` says, to listen by: keeping only what `hears` reads
    pub fn hearing(&self) -> impl Fn(&Town, &Word) -> bool + 'static {
        let (kinds, at) = (self.kinds.clone(), self.at.clone());
        let me = Resident { id: self.id, square: self.square, kinds, at, ..Resident::default() };
        move |town, w| me.hears(town, w)
    }

    /// What it hears, from now, as one coming (`Words::coming`): until it is told, only `WAITING`
    /// words wait.
    pub fn listening(&self, town: &Town) -> Listening {
        let mut words = town.words.coming(WAITING, self);
        words.hear(town, self.hearing());
        words
    }

    /// whether the word is wanted of it: a future word where it stands, joined, and no join nearer
    fn wanted(&self, town: &Town, w: &Word) -> bool {
        w.tense == Tense::Future
            && self.at.is_some()
            && town.joined(&w.kind).as_deref() == self.at.as_deref()
    }

    /// the facts now it is told as it starts, as words, by kind, each by its path from where it
    /// stands: of its kinds, and, standing at a kind, at it and under it
    fn present(&self, town: &Town) -> Vec<Word> {
        let here = self.at.iter().map(|at| format!("/{at}"));
        let mut present = std::collections::BTreeMap::new();
        for kind in self.kinds.iter().cloned().chain(here) {
            present.extend(town.facts(&kind));
        }
        let word = |(kind, body)| self.placed(Word::new(kind, Tense::Present, body));
        present.into_iter().map(word).collect()
    }

    /// a word as it is told it: by its kind's path from where it stands
    fn placed(&self, w: Word) -> Word {
        match &self.at {
            Some(at) if hears(&format!("/{at}"), &w.kind) => {
                let path = w.kind.strip_prefix(at.as_str()).unwrap_or("").trim_start_matches('/');
                Word { kind: path.to_string(), ..w }
            }
            _ => w,
        }
    }
}

/// Why a resident does not come to live: it stops speaking before it says what it is; what it
/// says it is cannot be, and why; or it stops speaking, or the town stops, as it waits for its
/// languages.
pub enum Unsaid {
    Mute,
    Refused(String),
    Gone,
}

/// A resident ready to live: it has said what it is, and its languages are said. What it owns
/// is its own while this lasts. It hears in the language of its name as it is now, not as it was
/// when it came.
pub struct Life<R> {
    resident: Resident,
    words: Listening,
    speech: Speech<R>,
    hears: String,
    _here: Option<Here>,
}

/// A resident, from its first line, what it is, to its languages said. It is heard from the moment
/// it comes, so a fact is heard once; until it is told, only so many words wait, since it may never
/// be. `waits` is told of each language not said yet.
pub async fn comes<R: AsyncRead + Unpin>(
    town: &Rc<Town>,
    who: &Resident,
    mut voice: Voice<R>,
    waits: impl AsyncFnMut(&str),
) -> Result<Life<R>, Unsaid> {
    let mut words = town.words.coming(WAITING, who);
    let first = voice.next().await.ok_or(Unsaid::Mute)?;
    let said: Resident = serde_json::from_str(&first)
        .map_err(|e| Unsaid::Refused(format!("its first line must say what it is: {e}")))?;
    let resident = Resident {
        kinds: said.kinds,
        own: said.own,
        speaks: said.speaks,
        languages: said.languages,
        ..who.clone()
    };
    words.hear(town, resident.hearing());
    let here = town.come(&resident).map_err(Unsaid::Refused)?;
    let (says, hears) = resident.languages();
    let (says, hears) = (says.to_string(), hears.to_string());
    let resident = Life::speaking(town, resident, words, voice, (&says, &hears), waits).await;
    Ok(Life { _here: Some(here), ..resident.ok_or(Unsaid::Gone)? })
}

impl<R: AsyncRead + Unpin> Life<R> {
    /// One that hears as `resident` does, once its languages are said. Until then what it says is
    /// heard while it is words in JSON, so it may say its language itself; from the first that is
    /// not, all is held for its language to read. `waits` is told of each language not said yet.
    /// Nothing if it stops speaking, or the town stops, first.
    pub async fn speaking(
        town: &Town,
        resident: Resident,
        words: Listening,
        mut voice: Voice<R>,
        (says, hears): (&str, &str),
        mut waits: impl AsyncFnMut(&str),
    ) -> Option<Life<R>> {
        let who = &resident.clone();
        let waiting = async {
            town.language(says, &mut waits).await?;
            town.language(hears, &mut waits).await
        };
        tokio::pin!(waiting);
        let mut in_json = true;
        let at = who.at.as_deref().unwrap_or("");
        // a word, read as it is once it lives, and who among those it speaks for says it
        let word = |line: &str| said(line).and_then(|(w, by)| Ok((placed(at, w)?, by))).ok();
        loop {
            let json_now = in_json;
            let said = async {
                match json_now {
                    true => voice.line_if(word).await,
                    false => {
                        voice.held().await;
                        None
                    }
                }
            };
            tokio::select! {
                biased;
                languages = &mut waiting => break languages.map(drop)?,
                said = said => match said {
                    Some(Some((w, by))) => match by_whom(town, who, by) {
                        Ok(by) => town.said(w, &by),
                        Err(e) => town.error(who, Some(&w.kind), e),
                    },
                    Some(None) => in_json = false,
                    None => return None,
                },
            }
        }
        let speech = Speech::new(voice, says);
        Some(Life { resident, words, speech, hears: hears.into(), _here: None })
    }

    /// It lives, both ways at once: what it says comes in by `Town::said`, and what it hears is
    /// told `to` by `tell`. One that stops reading is still heard. A program lives until `end`: one
    /// that stops saying is still told, and what it said is heard to the end of its output, but
    /// `LAST` after its end at most. A socket, with no `end`, lives until it stops saying.
    pub async fn live(
        self,
        town: &Town,
        to: &mut (impl AsyncWrite + Unpin),
        end: Option<impl std::future::Future>,
    ) {
        let Life { resident, words, mut speech, hears, _here } = self;
        let saying = async {
            while let Some((w, by)) = speech.said(town, &resident).await {
                town.said(w, &by);
            }
        };
        let telling = async {
            // in its language as it is now: none, while none speaks it, and what it is told cannot
            // be said in it
            let to = async |w: &Word| match town.languages.get(&hears) {
                Ok(hears) => say(to, w, &hears, &town.languages).await,
                Err(_) => Ok(false),
            };
            tell(town, &resident, words, to).await;
            std::future::pending::<()>().await
        };
        let Some(end) = end else {
            return tokio::select! {
                _ = saying => {}
                _ = telling => {}
            };
        };
        // told until it ends, though it stops saying; and what it said before its end is heard
        // to the end of its output, but `LAST` at most: what it started may hold that open
        let ended = async {
            tokio::select! {
                _ = end => {}
                _ = telling => {}
            }
        };
        tokio::pin!(saying, ended);
        tokio::select! {
            biased;
            _ = &mut saying => ended.await,
            _ = &mut ended => {
                let _ = tokio::time::timeout(crate::program::LAST, saying).await;
            }
        }
    }
}

/// How many words may wait for a resident that stops reading: all one may cost town. Those after
/// are dropped, and that is said, once each time it falls behind.
pub(crate) const WAITING: usize = 1024;

/// Tell a resident what it hears, by `to`, until `to` fails or the town ends.
///
/// 1. the present of its kinds, as it is now, whole;
/// 2. `now`;
/// 3. each word it hears since it came, but a fact that came before, as the present holds it.
///
/// - **more than may wait**: what waited is told still, and that it may have missed what came
///   after is said.
/// - **what `to` cannot hold**: `false`, passed over, unless it is wanted of it: then a
///   `town/error` from it.
/// - **another's error**: told last, what it says now may answer it (`Town::telling`).
pub async fn tell(
    town: &Town,
    resident: &Resident,
    mut words: Listening,
    mut to: impl AsyncFnMut(&Word) -> std::io::Result<bool>,
) {
    // however it ends, it is told no more
    struct Telling<'a>(&'a Town, u64);
    impl Drop for Telling<'_> {
        fn drop(&mut self) {
            self.0.telling(self.1, None);
        }
    }
    let _telling = Telling(town, resident.id);
    if words.told() {
        town.error(resident, None, "may have missed words before it was told".into());
    }
    for w in resident.present(town).into_iter().chain([Word::now()]) {
        if to(&w).await.is_err() {
            return;
        }
    }
    while let Some(w) = words.recv().await {
        town.telling(resident.id, Some(&w));
        let (wanted, kind) = (resident.wanted(town, &w), w.kind.clone());
        match to(&resident.placed(w)).await {
            Ok(true) => {}
            Ok(false) if wanted => {
                town.error(resident, Some(&kind), "cannot be said in its language".into())
            }
            Ok(false) => {}
            Err(_) => return,
        }
    }
}

/// The most a line or a unit may take, or what is held for a language not said yet: what is past
/// it is let go, and a header that says more is not of its language, so none can make town hold
/// without end.
#[cfg(not(test))]
const MOST: usize = 16 * 1024 * 1024;
/// less in a test, so it is quickly reached: still more than a read holds at once
#[cfg(test)]
const MOST: usize = 64 * 1024;

/// A resident's voice: what it says, as it comes, a line at a time, or a unit of a language at a
/// time, before it is words (`Speech`). A line is text whatever bytes it holds: what is not UTF-8
/// reads as �, so one bad line is a line like any other, and what comes after it is heard. Safe to
/// wait on in a `select!`: what is half read is kept for the next wait.
pub struct Voice<R> {
    from: tokio::io::BufReader<R>,
    held: Vec<u8>,
    ended: bool,
    /// a line is past `MOST`: what is left of it is let go
    over: bool,
    /// how many times a unit has been read, for a test to see the work bounded
    #[cfg(test)]
    tries: usize,
}

impl<R: AsyncRead + Unpin> Voice<R> {
    pub fn new(r: R) -> Self {
        let from = tokio::io::BufReader::new(r);
        #[cfg(test)]
        let voice = Voice { from, held: Vec::new(), ended: false, over: false, tries: 0 };
        #[cfg(not(test))]
        let voice = Voice { from, held: Vec::new(), ended: false, over: false };
        voice
    }

    /// The next line, without its end; nothing once the stream has ended or failed. A line held
    /// already, read as a unit was looked for, is the next. A line is at most `MOST`: what is past
    /// it is let go, to its end.
    pub async fn next(&mut self) -> Option<String> {
        // how far is looked at for a line's end: each byte once, however long the line
        let mut seen = 0;
        loop {
            let end = self.held[seen..].iter().position(|&c| c == b'\n').map(|n| seen + n);
            seen = self.held.len();
            match end {
                Some(n) if self.over => {
                    self.held.drain(..=n);
                    self.over = false;
                    seen = 0;
                    continue;
                }
                None if self.over => {
                    self.held.clear();
                    seen = 0;
                }
                Some(n) => return Some(self.line(n + 1)),
                None if self.held.len() >= MOST => {
                    self.over = true;
                    return Some(self.line(MOST));
                }
                None if self.ended => {
                    return (!self.held.is_empty()).then(|| self.line(self.held.len()))
                }
                None => {}
            }
            if self.ended {
                return None;
            }
            self.more().await;
        }
    }

    /// the first `n` bytes held, as a line, without its end
    fn line(&mut self, n: usize) -> String {
        let mut line: Vec<u8> = self.held.drain(..n).collect();
        let end = [&b"\r\n"[..], b"\n"].into_iter().find(|e| line.ends_with(e));
        line.truncate(line.len() - end.map_or(0, |e| e.len()));
        String::from_utf8_lossy(&line).into_owned()
    }

    /// what comes, held as far as `MOST`, until the end
    async fn held(&mut self) {
        while !self.ended {
            self.more().await;
            self.held.truncate(MOST);
        }
    }

    /// the next unit, as `unit_held` reads it, taken
    #[cfg(test)]
    pub async fn unit(
        &mut self,
        language: &str,
        languages: &Languages,
    ) -> Option<Result<Parts, Vec<u8>>> {
        Some(match self.unit_held(language, languages).await? {
            Ok((p, n)) => {
                self.held.drain(..n);
                Ok(p)
            }
            Err(bytes) => Err(bytes),
        })
    }

    /// The next unit said in the language named `language`, as it is at each read; nothing once the
    /// stream has ended.
    ///
    /// - **of it**: its parts, and how many of the held bytes it is read from, still held.
    /// - **none speaks it**: no bytes, for the caller to wait for it.
    /// - **not of it**: those bytes, so what comes after is heard: to their line's end, or, with
    ///   none, to where it is of the language again, as a binary language has no lines.
    /// - **waiting**: a unit is read again only once as many bytes as it needs have come, and what
    ///   is held has grown by half since it was last read, or the stream is quiet (`wait`); none
    ///   takes more than `MOST`.
    async fn unit_held(
        &mut self,
        language: &str,
        languages: &Languages,
    ) -> Option<Result<(Parts, usize), Vec<u8>>> {
        // what was held as it was last read: each read is of half as much again, so all the reads
        // of one unit cost what three reads of it whole would
        let mut tried = 0;
        loop {
            let Ok(language) = languages.get(language) else { return Some(Err(Vec::new())) };
            let language = &*language;
            // the end, but for blank lines in JSON, which are nothing
            let blank = matches!(language, Language::Json) && self.held.trim_ascii().is_empty();
            if self.ended && (self.held.is_empty() || blank) {
                return None;
            }
            #[cfg(test)]
            {
                self.tries += 1;
            }
            match language.read(&self.held, self.ended, languages) {
                Read::Got(p, n) if n > 0 => return Some(Ok((p, n))),
                Read::More(need) if need <= MOST => {
                    let grown = tried + tried / 2;
                    tried = self.held.len();
                    self.wait(need, grown).await
                }
                // nothing held is not yet anything: what comes may be of it
                _ if self.held.is_empty() => self.more().await,
                // not of it, of it but empty, or more than a unit may take: let go
                read => {
                    let n = match self.held.iter().position(|&c| c == b'\n') {
                        Some(n) => n + 1,
                        None => self.again(language, languages, read),
                    };
                    return Some(Err(self.held.drain(..n).collect()));
                }
            }
        }
    }

    /// Where what is held is of `language` again, after `read`, which is not: each try begins
    /// past as far as the one before looked, so each byte is looked at once; all held, if it is
    /// not of it anywhere. The trade: a unit that begins within what a failed read looked at is
    /// not tried, and is let go with the bytes before it.
    fn again(&self, language: &Language, languages: &Languages, mut read: Read) -> usize {
        let mut at = 0;
        loop {
            at += match read {
                Read::No(far) => far.max(1),
                _ => 1,
            };
            if at >= self.held.len() {
                return self.held.len();
            }
            read = language.read(&self.held[at..], self.ended, languages);
            match read {
                Read::Got(_, n) if n > 0 => return at,
                Read::More(need) if need <= MOST => return at,
                _ => {}
            }
        }
    }

    /// The next line, as `take` makes it, if it takes it; if not, nothing is taken, so it is held
    /// for another language to read. A blank line is nothing. `None` once the stream has ended.
    pub async fn line_if<T>(&mut self, take: impl Fn(&str) -> Option<T>) -> Option<Option<T>> {
        // how far is looked at for a line's end: each byte once, however long the line
        let mut seen = 0;
        loop {
            let end = self.held[seen..].iter().position(|&c| c == b'\n').map(|n| seen + n);
            seen = self.held.len();
            let n = match end {
                Some(n) => n,
                None if self.ended && self.held.trim_ascii().is_empty() => return None,
                None if self.ended => self.held.len(),
                None if self.held.len() >= MOST => return Some(None),
                None => {
                    self.more().await;
                    continue;
                }
            };
            let line = String::from_utf8_lossy(&self.held[..n]).into_owned();
            let line = line.trim_end_matches('\r');
            if line.trim().is_empty() {
                self.held.drain(..(n + 1).min(self.held.len()));
                seen = 0;
                continue;
            }
            let taken = take(line);
            if taken.is_some() {
                self.held.drain(..(n + 1).min(self.held.len()));
            }
            return Some(taken);
        }
    }

    /// As many bytes held as a unit needs, or the end: more is more, a byte at least, whatever a
    /// grammar says it needs. Then on to `grown`, as far as `MOST`, while more comes: a grammar
    /// that cannot say what it needs says a byte more, and read again for each would read a unit
    /// again from its start each time a little comes. What comes and stops, `QUIET`, is read at
    /// once, as one waits for an answer to what it said.
    async fn wait(&mut self, need: usize, grown: usize) {
        let need = need.max(self.held.len() + 1);
        while self.held.len() < need && !self.ended {
            self.more().await;
        }
        while self.held.len() < grown.min(MOST) && !self.ended {
            if tokio::time::timeout(QUIET, self.more()).await.is_err() {
                return;
            }
        }
    }

    /// what comes next, held; or the end
    async fn more(&mut self) {
        use tokio::io::AsyncBufReadExt;
        match self.from.fill_buf().await {
            Ok(b) if !b.is_empty() => {
                let n = b.len();
                self.held.extend_from_slice(b);
                self.from.consume(n);
            }
            _ => self.ended = true,
        }
    }
}

/// How long a stream is still before what it holds is read again, though less has come than
/// `Voice::wait` waits for.
const QUIET: std::time::Duration = std::time::Duration::from_millis(5);

/// What a resident says, as words in its language, in its `Voice`: in JSON, a blank line is
/// nothing; and what is no word is a `town/error` from whoever said it. Each kind is a path from
/// where its sayer stands. Its language is the one of its name as it is now: while none speaks it,
/// what it says waits. Safe to wait on in a `select!`, as its voice is.
pub struct Speech<R> {
    voice: Voice<R>,
    language: String,
}

impl<R: AsyncRead + Unpin> Speech<R> {
    /// what its `voice` says, in the language named `language`, after what it has said already
    pub fn new(voice: Voice<R>, language: &str) -> Self {
        Speech { voice, language: language.into() }
    }

    /// the next word said, by `from`, where it stands; nothing once it has stopped speaking
    #[cfg(test)]
    pub async fn next(&mut self, town: &Town, from: &Resident) -> Option<Word> {
        self.said(town, from).await.map(|(w, _)| w)
    }

    /// The next word said, and by whom: `from`, or the one it speaks for that the word names `by`.
    /// Nothing once it has stopped speaking.
    pub async fn said(&mut self, town: &Town, from: &Resident) -> Option<(Word, Resident)> {
        let at = from.at.as_deref().unwrap_or("");
        loop {
            // none speaks it now: what it says waits for it
            let language = match town.languages.get(&self.language) {
                Ok(language) => language,
                Err(_) => {
                    let waits = async |message: &str| town.error(from, None, message.into());
                    town.language(&self.language, waits).await?
                }
            };
            // what is no word is said as the bytes it came in, so whoever said it knows it
            let (said, why) = if let Language::Json = *language {
                let line = self.voice.next().await?;
                if line.trim().is_empty() {
                    continue;
                }
                match said(&line).and_then(|(w, by)| Ok((placed(at, w)?, by))) {
                    Ok((w, by)) => match by_whom(town, from, by) {
                        Ok(who) => return Some((w, who)),
                        Err(e) => (line, e),
                    },
                    Err(e) => (line, e),
                }
            } else {
                match self.voice.unit_held(&self.language, &town.languages).await? {
                    Ok((p, n)) => {
                        let word =
                            spoken(at, &p).and_then(|(w, by)| Ok((w, by_whom(town, from, by)?)));
                        let said: Vec<u8> = self.voice.held.drain(..n).collect();
                        match word {
                            Ok(said) => return Some(said),
                            Err(e) => (String::from_utf8_lossy(&said).into_owned(), e),
                        }
                    }
                    Err(bytes) if bytes.is_empty() => continue,
                    Err(bytes) => {
                        let why = "not of its language".to_string();
                        (String::from_utf8_lossy(&bytes).into_owned(), why)
                    }
                }
            };
            town.misheard(from, said.trim_end_matches('\n'), format!("not a word: {why}"));
        }
    }
}

/// Who says a word `from` says: itself, or the resident it speaks for named `by`.
fn by_whom(town: &Town, from: &Resident, by: Option<String>) -> Result<Resident, String> {
    match by {
        None => Ok(from.clone()),
        Some(by) => town.spoken_for(from, &by),
    }
}

/// Parts said standing `at` a kind, or the root, as a word: `of`, then `placed`.
pub fn word_at(at: &str, p: &Parts) -> Result<Word, String> {
    of(p).and_then(|w| placed(at, w))
}

/// What a resident says, as parts: a word, and `by`, the one it speaks for that says it, if it
/// names one. `by` is the voice's, not the word's.
fn spoken(at: &str, p: &Parts) -> Result<(Word, Option<String>), String> {
    let Parts::Map(parts) = p else { return word_at(at, p).map(|w| (w, None)) };
    let by = p.get("by").map(|b| b.text().map(String::from).ok_or("`by` is a name")).transpose()?;
    let word: Vec<_> = parts.iter().filter(|(name, _)| name != "by").cloned().collect();
    Ok((word_at(at, &Parts::Map(word))?, by))
}

/// A word said standing `at` a kind, or the root: its kind is a path from there, `at/a/b` for
/// `a/b`, and `at` itself for none. One that names no kind, `a//b` or `../x`, is no word.
fn placed(at: &str, mut w: Word) -> Result<Word, String> {
    w.kind = match (at, w.kind.as_str()) {
        (at, "") => at.to_string(),
        ("", path) => path.to_string(),
        (at, path) => format!("{at}/{path}"),
    };
    match is_kind(&w.kind) {
        true => Ok(w),
        false => Err(format!("`{}` is no kind: {KIND}", w.kind)),
    }
}

/// Write a word to whoever hears it in `language`: the one place a word becomes bytes. A word the
/// language cannot hold is not written, and is `false`.
pub async fn say(
    to: &mut (impl AsyncWrite + Unpin),
    word: &Word,
    language: &Language,
    languages: &Languages,
) -> std::io::Result<bool> {
    match bytes(word, language, languages) {
        Some(b) => to.write_all(&b).await.map(|_| true),
        None => Ok(false),
    }
}

/// A word in `language`, as bytes: a line of JSON, or a unit of its language; nothing if the
/// language cannot hold it.
pub fn bytes(word: &Word, language: &Language, languages: &Languages) -> Option<Vec<u8>> {
    match language {
        Language::Json => Some((line(word) + "\n").into_bytes()),
        _ => language.bytes(&parts(word), languages),
    }
}

/// a word as parts: `kind`, `tense`, `body`, and `when` once it is said
pub(crate) fn parts(w: &Word) -> Parts {
    let mut p =
        Parts::map([("kind", Parts::from(w.kind.as_str())), ("tense", w.tense.name().into())]);
    p.set("body", w.body.parts().clone());
    if w.when != 0 {
        p.set("when", w.when);
    }
    p
}

/// What a word's `when` is, as town says when it is not one.
const WHEN: &str = "`when` is a whole number of ms, 0 or more";

/// What a word is made of, and nothing else.
const WORD: [&str; 4] = ["kind", "tense", "body", "when"];

/// parts as a word, of nothing but what a word is made of, read as `Line` reads it: a tense not
/// said is present
pub(crate) fn of(p: &Parts) -> Result<Word, String> {
    let Parts::Map(parts) = p else { return Err("a word is parts by name".into()) };
    if let Some((name, _)) = parts.iter().find(|(name, _)| !WORD.contains(&name.as_str())) {
        let is = "`kind`, `tense`, `body` and `when`";
        return Err(format!("a word has no `{name}`: it is {is}"));
    }
    // a word of no kind is where it is said: `placed` makes it a kind, or no word
    let kind = match p.get("kind") {
        None => "",
        Some(kind) => kind.text().ok_or("a kind is text")?,
    };
    let tense = match p.get("tense") {
        None => Tense::Present,
        Some(t) => t.text().and_then(Tense::named).ok_or("a tense is past, present or future")?,
    };
    let mut w = Word::new(kind, tense, Body::of(p.get("body").cloned().unwrap_or_default()));
    // as `Line` reads it: a whole number, 0 or more
    w.when = match p.get("when") {
        None => 0,
        Some(Parts::Number(n)) => n.parse().map_err(|_| WHEN)?,
        Some(_) => return Err(WHEN.into()),
    };
    Ok(w)
}

/// A word as a line of JSON, as the log and every resident that speaks no other say it.
///
/// - **what it holds**: `WORD` and nothing else, as `of` reads parts; and, said by a resident, `by`
///   (`spoken`).
/// - **its body**: kept as the bytes it came in, read only if a part is asked for.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Line<'a> {
    #[serde(borrow, default)]
    kind: Cow<'a, str>,
    #[serde(borrow, default)]
    tense: Option<Cow<'a, str>>,
    #[serde(borrow, default)]
    body: Option<&'a RawValue>,
    #[serde(default)]
    when: u64,
    #[serde(borrow, default)]
    by: Option<Cow<'a, str>>,
}

/// a line, as a word: of nothing but what a word is made of
pub fn read(line: &str) -> Result<Word, String> {
    match said(line)? {
        (w, None) => Ok(w),
        (_, Some(_)) => Err("a word has no `by`: it is `kind`, `tense`, `body` and `when`".into()),
    }
}

/// a line a resident says, as a word, and `by`, the one it speaks for that says it, if it names one
fn said(line: &str) -> Result<(Word, Option<String>), String> {
    let l: Line = serde_json::from_str(line).map_err(|e| e.to_string())?;
    let tense = match l.tense {
        None => Tense::Present,
        Some(t) => Tense::named(&t).ok_or("a tense is past, present or future")?,
    };
    let body = l.body.map_or_else(Body::none, |b| Body::said(b.get().as_bytes()));
    let by = l.by.map(Cow::into_owned);
    Ok((Word { kind: l.kind.into_owned(), tense, body, when: l.when, by: 0 }, by))
}

/// a word, as a line
pub fn line(w: &Word) -> String {
    let body = w.body.json();
    let kind = serde_json::to_string(&w.kind).unwrap_or_default();
    let mut line = format!(r#"{{"kind":{kind},"tense":"{}","body":"#, w.tense.name());
    line.push_str(&String::from_utf8_lossy(&body));
    if w.when != 0 {
        line.push_str(&format!(r#","when":{}"#, w.when));
    }
    line.push('}');
    line
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{guard, run, scratch};

    /// what one is told, its bodies, `now` among them as none: once told `now`, a past `end` is
    /// said, and hearing it, it hangs up
    async fn told(town: &Town, resident: &Resident, words: Listening) -> Vec<Body> {
        let (mut told, end) = (Vec::new(), Body::of("end"));
        let to = async |w: &Word| {
            if w.body == end {
                return Err(std::io::ErrorKind::BrokenPipe.into());
            }
            if w.is_now() {
                town.talk(Word::new("theme", Tense::Past, end.clone()));
            }
            told.push(w.body.clone());
            Ok(true)
        };
        tell(town, resident, words, to).await;
        told
    }

    // one slow to be told, as more is said than may wait, is not said to fall behind; told, it
    // hears the present as it is then, not as it was when it came, then what waited, and is said
    // to have missed what came after
    #[test]
    fn one_slow_to_be_told_hears_the_present_now() {
        run(async {
            let dir = scratch();
            let town = Town::with_log(dir.log());
            let mut errors = town.words.hear(|_, w| w.kind == "town/error", None);
            town.talk(Word::new("theme", Tense::Present, Body::of("dark")));
            let who = town.who("slow", true);
            let resident = Resident { kinds: vec!["theme".into()], ..who.clone() };
            let coming = || resident.listening(&town);
            // as many as may wait: the present as it is, and what is not a fact since
            let words = coming();
            town.talk(Word::new("theme", Tense::Present, Body::of("dim")));
            town.talk(Word::new("theme", Tense::Past, Body::of("dimmed")));
            assert_eq!(
                told(&town, &resident, words).await,
                [Body::of("dim"), Body::none(), Body::of("dimmed")],
                "the fact as it is, not as it was, and not told again"
            );
            // more: the present as it is, then what waited, though more after it may be missed
            let words = coming();
            town.talk(Word::new("theme", Tense::Past, Body::of("first")));
            for _ in 0..WAITING {
                town.talk(Word::new("theme", Tense::Past, Body::none()));
            }
            town.talk(Word::new("theme", Tense::Present, Body::of("light")));
            assert!(errors.try_recv().is_err(), "not said to fall behind before it is told");
            let mut heard = Vec::new();
            let to = async |w: &Word| {
                heard.push(w.body.clone());
                match heard.len() < 3 {
                    true => Ok(true),
                    false => Err(std::io::ErrorKind::BrokenPipe.into()),
                }
            };
            tell(&town, &resident, words, to).await;
            assert_eq!(heard, [Body::of("light"), Body::none(), Body::of("first")], "what waited");
            let missed = errors.try_recv().expect("said to have missed words");
            assert_eq!(missed.body.value()["message"], "may have missed words before it was told");
        });
    }

    // in JSON a blank line is nothing, between words and after the last
    #[test]
    fn a_blank_line_is_nothing() {
        run(async {
            let mut voice = Voice::new(&b"\n{\"a\":1}\n\n \n"[..]);
            let languages = Languages::default();
            let a = voice.unit("json", &languages).await;
            assert_eq!(a, Some(Ok(Parts::map([("a", Parts::from(1i64))]))));
            assert_eq!(voice.unit("json", &languages).await, None);
        });
    }

    // a language that reads nothing of nothing waits for what comes, rather than calling nothing
    // astray again and again
    #[test]
    fn nothing_held_is_waited_on() {
        run(async {
            let languages = Languages::default();
            let dangling = crate::language::Grammar::of(&Parts::map([("ref", "none")])).unwrap();
            languages.say("dangling", dangling);
            let (mut to, from) = tokio::io::duplex(64);
            tokio::task::spawn_local(async move {
                tokio::task::yield_now().await;
                to.write_all(b"x").await.unwrap();
            });
            let mut voice = Voice::new(from);
            assert_eq!(voice.unit("dangling", &languages).await, Some(Err(b"x".to_vec())));
            assert_eq!(voice.unit("dangling", &languages).await, None);
        });
    }

    // a line that is not UTF-8 is a line like any other, and those after it are heard; a last one
    // without its end is heard too
    #[test]
    fn lines_are_read_whatever_they_hold() {
        run(async {
            let mut voice = Voice::new(&b"one\r\nbad \xff here\n{\"kind\":\"a\"}\nlast"[..]);
            let mut said = Vec::new();
            while let Some(line) = voice.next().await {
                said.push(line);
            }
            assert_eq!(said, ["one", "bad \u{fffd} here", "{\"kind\":\"a\"}", "last"]);
        });
    }

    // what is said is heard as words: a blank line is nothing, one that is no word is an
    // `town/error` from whoever said it, and what comes after either is heard
    #[test]
    fn speech_is_words() {
        run(async {
            let dir = scratch();
            let town = Town::with_log(dir.log());
            let mut heard = town.words.listen();
            let said = &b"{\"kind\":\"a\"}\n\n{\"body\":1}\n{\"kind\":\"b\"}\n"[..];
            let mut speech = Speech::new(Voice::new(said), "json");
            let me = Resident::town();
            assert_eq!(speech.next(&town, &me).await.unwrap().kind, "a");
            let b = speech.next(&town, &me).await.unwrap();
            assert_eq!(b.kind, "b", "past the blank and the bad");
            assert!(speech.next(&town, &me).await.is_none());
            let e = heard.try_recv().expect("the bad line is an error");
            assert_eq!(
                (e.kind.as_str(), e.body.value()["from"].as_str()),
                ("town/error", Some("town"))
            );
            assert!(e.body.value()["message"].as_str().unwrap().starts_with("not a word"));
            assert_eq!(e.body.value()["said"], "{\"body\":1}", "what was said, to know it by");
            assert!(heard.try_recv().is_err(), "and the blank line is nothing");
        });
    }

    // a word is what it is made of, and nothing else, in JSON and in parts alike: what has more
    // is no word, as a typo is no tense
    #[test]
    fn a_word_is_exactly_its_parts() {
        assert!(read(r#"{"kind":"a","tense":"past","body":1,"when":2}"#).is_ok());
        // `when` is a whole number, 0 or more, read so in JSON and in parts alike
        for (json, when) in
            [("1.5", Parts::from(1.5)), ("-5", (-5i64).into()), ("\"5\"", "5".into())]
        {
            assert!(read(&format!(r#"{{"kind":"a","when":{json}}}"#)).is_err(), "{json}");
            let w = Parts::map([("kind", Parts::from("a")), ("when", when)]);
            assert!(of(&w).is_err(), "{json}");
        }
        let w = Parts::map([("kind", Parts::from("a")), ("when", Parts::from(7i64))]);
        assert_eq!(of(&w).unwrap().when, 7);
        assert!(read(r#"{"x":1}"#).unwrap_err().contains("unknown field `x`"));
        assert!(read(r#"{"kind":"a","tenses":"past"}"#).unwrap_err().contains("`tenses`"));
        let x = Parts::map([("x", Parts::from(1i64))]);
        assert_eq!(
            of(&x).unwrap_err(),
            "a word has no `x`: it is `kind`, `tense`, `body` and `when`"
        );
        assert!(word_at("", &Parts::map([("tense", "past")])).is_err(), "at the root, a kind");
        assert_eq!(word_at("box", &Parts::map([("tense", "past")])).unwrap().kind, "box");
        // `by` is what a resident says of a word, not the word: read anywhere else, it is refused
        assert!(read(r#"{"kind":"a","by":"sub"}"#).unwrap_err().contains("no `by`"));
        let by = Parts::map([("kind", Parts::from("a")), ("by", Parts::from("sub"))]);
        assert!(of(&by).is_err(), "nor is it a word's in parts");
        let (w, by) = spoken("", &by).unwrap();
        assert_eq!((w.kind.as_str(), by.as_deref()), ("a", Some("sub")), "a resident says it");
    }

    // a line looked at, and taken, is gone; one not taken is held, the next line, not lost or run
    // into the one after: as a resident waits for its language, it is read as a word is once it
    // lives, so a body read whole is taken, and a `when` that is no whole number is not
    #[test]
    fn a_line_held_is_the_next() {
        run(async {
            let said = b"{\"kind\":\"a\",\"body\":1e400}\n{\"kind\":\"b\",\"when\":1.5}\n";
            let mut voice = Voice::new(&said[..]);
            let first = voice.line_if(|l| read(l).ok()).await;
            assert_eq!(first.flatten().map(|w| w.kind), Some("a".into()));
            let second = voice.line_if(|l| read(l).ok()).await;
            assert!(matches!(second, Some(None)), "not taken");
            assert_eq!(voice.next().await.as_deref(), Some(r#"{"kind":"b","when":1.5}"#));
            assert_eq!(voice.next().await, None);
        });
    }

    // a line is at most `MOST`: what is past it is let go, to its end, and the next is heard
    #[test]
    fn a_line_is_at_most_most() {
        run(async {
            let mut said = vec![b'a'; MOST + 10];
            said.extend_from_slice(b"\nnext\n");
            let mut voice = Voice::new(&said[..]);
            assert_eq!(voice.next().await.map(|l| l.len()), Some(MOST));
            assert_eq!(voice.next().await.as_deref(), Some("next"));
            assert_eq!(voice.next().await, None);
        });
    }

    // a unit that comes in many pieces is read again only as what is held grows by half, not for
    // each piece, so its reads cost what a few of it whole would; and one that comes and stops is
    // read, though less came than was waited for
    #[test]
    fn a_unit_is_read_again_as_it_grows() {
        run(async {
            let languages = Languages::default();
            let line = crate::language::Grammar::of(&Parts::map([("upto", "\n")])).unwrap();
            languages.say("line", line);
            let (mut to, from) = tokio::io::duplex(256);
            let said = MOST - 1;
            tokio::task::spawn_local(async move {
                for _ in 0..said / 64 {
                    to.write_all(&[b'a'; 64]).await.unwrap();
                }
                to.write_all(b"\n").await.unwrap();
                // one more, and the stream still open: it is read, though no more comes
                to.write_all(b"b\n").await.unwrap();
                std::future::pending::<()>().await
            });
            let mut voice = Voice::new(from);
            let got = voice.unit("line", &languages).await.unwrap().unwrap();
            assert_eq!(got.text().map(str::len), Some(said / 64 * 64));
            assert!(voice.tries < 40, "read {} times, for {} pieces", voice.tries, said / 64);
            let next = guard(voice.unit("line", &languages)).await;
            assert_eq!(next, Some(Ok(Parts::from("b"))), "what came and stopped, read");
        });
    }

    // in a language not JSON, a unit that is no word is said as the bytes it was read from, as
    // `town ask` knows its own by
    #[test]
    fn a_unit_that_is_no_word_is_said_as_it_came() {
        run(async {
            let dir = scratch();
            let town = Town::with_log(dir.log());
            let mut heard = town.words.listen();
            let grammar = serde_json::json!({ "seq": [{ "part": ["kind", { "upto": "\n" }] }] });
            town.languages
                .say("bare", crate::language::Grammar::of(&Parts::from(grammar)).unwrap());
            let mut speech = Speech::new(Voice::new(&b"a//b\nc\n"[..]), "bare");
            assert_eq!(speech.next(&town, &Resident::town()).await.unwrap().kind, "c");
            let e = heard.try_recv().expect("the unit that is no word is an error");
            assert_eq!(e.body.value()["said"], "a//b");
        });
    }

    // one waiting for its language that says what is not JSON, then stops speaking, is gone
    #[test]
    fn one_that_stops_as_it_waits_is_gone() {
        run(async {
            let dir = scratch();
            let town = Town::with_log(dir.log());
            let voice = Voice::new(&b"{\"kind\":\"a\"}\n\xff\xfe\n"[..]);
            let who = town.who("gone", true);
            let resident = Resident { kinds: Vec::new(), ..who.clone() };
            let words = resident.listening(&town);
            let languages = ("cbro", "cbro");
            let waits = async |_: &str| {};
            let waited = Life::speaking(&town, resident, words, voice, languages, waits);
            let waited = tokio::time::timeout(std::time::Duration::from_secs(5), waited);
            assert!(waited.await.expect("it ends").is_none());
        });
    }

    // a word said is a line of it
    #[test]
    fn a_word_is_said_as_a_line() {
        run(async {
            let mut out = Vec::new();
            say(
                &mut out,
                &Word::new("a", Tense::Past, Body::none()),
                &Language::Json,
                &Languages::default(),
            )
            .await
            .unwrap();
            assert_eq!(out, b"{\"kind\":\"a\",\"tense\":\"past\",\"body\":null}\n");
        });
    }

    // a line read is written as it came, its body unread; a tense not said is present; and `at`,
    // which is no part of a word, or a tense that is none, is no word
    #[test]
    fn a_line_goes_on_as_it_came() {
        let said = r#"{"kind":"a","tense":"past","body":{ "b" : [1,  2] },"when":5}"#;
        let w = read(said).unwrap();
        assert_eq!((w.tense, w.when), (Tense::Past, 5));
        assert_eq!(line(&w), said);
        assert!(read(r#"{"kind":"a","at":7}"#).is_err(), "a word has `when`, not `at`");
        let w = read(r#"{"kind":"\u0061","body":{"who":"me"},"when":7}"#).unwrap();
        assert_eq!((w.kind.as_str(), w.tense, w.when), ("a", Tense::Present, 7));
        assert_eq!(w.body.part("who").and_then(Parts::text), Some("me"));
        assert!(read(r#"{"kind":"a","tense":"soon"}"#).is_err());
    }
}
