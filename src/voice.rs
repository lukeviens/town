//! A voice: how a resident speaks. Here alone bytes become words, and words bytes: what it says is
//! read in its language, each kind a path from where it stands, and goes in by the one door,
//! `Town::said`; it is told by `tell`.

use std::borrow::Cow;
use std::rc::Rc;

use serde::Deserialize;
use serde_json::value::RawValue;
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};

use crate::body::Body;
use crate::language::{Language, Languages, Read};
use crate::parts::Parts;
use crate::town::{Here, Listening, Resident, To, Town};
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
    /// Whether it is told this word, for itself: of its kinds, its own among them, so each hears in
    /// town's one order; and, joined, each future word at its kind or under it, unless a nearer
    /// join stands there. Never its own errors, each of which could make another, but one at the
    /// square, whatever it names: it has no other way to be told. Never one said in answer to an
    /// error. What those it speaks for hear, it is told `by` them (`tells`).
    pub fn hears(&self, town: &Town, w: &Word) -> bool {
        if w.kind == crate::word::ERROR {
            // its own error, by who it is, not by name, which many may go by
            if w.by != 0 && w.by == self.id {
                return self.square;
            }
            // one said in answer to an error, none that hears errors is told: so none answers it
            if w.body.part("answering") == Some(&Parts::Bool(true)) {
                return false;
            }
        }
        self.wanted(town, w) || self.names(&w.kind)
    }

    /// Whether its kinds name `kind`, as any list of patterns does (`heard`): but not, come by a
    /// name, what passes through the name, which came by it, or by it from one it reaches.
    fn names(&self, kind: &str) -> bool {
        let passes = self.visitor && crate::word::through(&self.name, kind);
        crate::word::heard(&self.kinds, &self.except, kind) && !passes
    }

    /// Whom a word is for, of it and those it speaks for, as each `hears`, to listen by: itself,
    /// and each it speaks for, by name, as it is when the word is said. Keeping only what `hears`
    /// reads.
    pub fn tells(&self) -> impl Fn(&Town, &Word) -> Vec<To> + 'static {
        let (kinds, except, at) = (self.kinds.clone(), self.except.clone(), self.at.clone());
        let (id, square, visitor, name) = (self.id, self.square, self.visitor, self.name.clone());
        let me = Resident { id, square, visitor, name, kinds, except, at, ..Resident::default() };
        move |town, w| {
            let spoken = town.spoken_by(id);
            let spoken = spoken.iter().filter(|(_, r)| r.hears(town, w));
            let it = me.hears(town, w).then_some(To::It);
            it.into_iter().chain(spoken.map(|(name, _)| To::One(name.clone()))).collect()
        }
    }

    /// What it hears, from now, as one coming (`Words::coming`): until it is told, only as many
    /// words wait as it says (`waits`).
    pub fn listening(&self, town: &Town) -> Listening {
        let mut words = town.words.coming(self);
        words.waits(self.waits);
        words.tell(town, self.tells());
        words
    }

    /// whether the word is wanted of it: a future word where it stands, joined, and no join nearer
    fn wanted(&self, town: &Town, w: &Word) -> bool {
        w.tense == Tense::Future
            && self.at.is_some()
            && town.joined(&w.kind).as_deref() == self.at.as_deref()
    }

    /// the facts now it is told as it starts, as words, by kind, each by its path from where it
    /// stands: of its kinds. Not, standing at a kind, what is there: it says that itself.
    pub(crate) fn present(&self, town: &Town) -> Vec<Word> {
        let mut present = std::collections::BTreeMap::new();
        for kind in &self.kinds {
            present.extend(town.facts(kind));
        }
        // what its kinds hear, but what it excepts
        present.retain(|kind, _| self.names(kind));
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
/// says it is cannot be, and why; or it stops speaking, ends, or the town stops, as it waits for
/// its name or its languages.
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
/// it comes, so a fact is heard once, and all it hears waits for it until it is told, as many as it
/// `waits` for once it says what it is. `waits` is told of each language not said yet.
pub async fn comes<R: AsyncRead + Unpin>(
    town: &Rc<Town>,
    who: &Resident,
    mut voice: Voice<R>,
    mut waits: impl AsyncFnMut(&str),
) -> Result<Life<R>, Unsaid> {
    let mut words = town.words.coming(who);
    let first = voice.next().await.ok_or(Unsaid::Mute)?;
    let resident = match named(who, &first) {
        Some(name) => {
            // while it waits for its name it is read too, so one that hangs up goes, and waits no
            // more; what it says meanwhile is held for when it lives
            let it = {
                let given = town.named(&name, &mut waits);
                tokio::pin!(given);
                loop {
                    tokio::select! {
                        it = &mut given => break it.ok_or(Unsaid::Gone)?,
                        _ = voice.more() => {}
                    }
                    if voice.ended {
                        return Err(Unsaid::Gone);
                    }
                }
            };
            // heard from now, as it comes: what was said while it waited was not to it
            words = town.words.coming(who);
            Resident { kinds: it.kinds, except: it.except, name, visitor: true, ..who.clone() }
        }
        None => {
            let said: Resident = crate::parts::json(first.as_bytes())
                .map_err(|e| Unsaid::Refused(format!("its first line must say what it is: {e}")))?;
            Resident {
                kinds: said.kinds,
                except: said.except,
                own: said.own,
                speaks: said.speaks,
                languages: said.languages,
                names: said.names,
                waits: said.waits,
                most: said.most,
                keep: said.keep,
                ..who.clone()
            }
        }
    };
    words.waits(resident.waits);
    words.tell(town, resident.tells());
    let here = town.come(&resident).map_err(Unsaid::Refused)?;
    let resident = Life::speaking(town, resident, words, voice, waits).await;
    Ok(Life { _here: Some(here), ..resident.ok_or(Unsaid::Gone)? })
}

/// The name one at the square comes by, if it comes by one, `{"name": "luke"}`, as another town
/// does: it is then what the resident that names it says, and nothing else.
fn named(who: &Resident, first: &str) -> Option<String> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Named {
        name: String,
    }
    let named: Named = crate::parts::json(first.as_bytes()).ok().filter(|_| who.square)?;
    Some(named.name)
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
        mut waits: impl AsyncFnMut(&str),
    ) -> Option<Life<R>> {
        let who = &resident.clone();
        // by name: JSON if it names none
        let (says, hears) = who.speaks.as_ref().map_or(("json", "json"), Speaks::names);
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
    /// that stops saying is still told, and what it said is heard to the end of its output. A
    /// socket, with no `end`, lives until it stops saying.
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
            let to = async |w: &Word, by: Option<&str>| match town.languages.get(&hears) {
                Ok(hears) => say(to, w, by, &hears, &town.languages, resident.most).await,
                Err(_) => Ok(false),
            };
            tell(town, &resident, words, to).await;
            std::future::pending::<()>().await
        };
        // told until it ends, though it stops saying; and what it said before its end is heard
        // to the end of its output
        let living = async {
            let Some(end) = end else { return saying.await };
            tokio::join!(saying, end);
        };
        // one that came by a name lives as long as the name is what it came by
        let named = async {
            match resident.visitor {
                true => {
                    let is = crate::town::Name {
                        kinds: resident.kinds.clone(),
                        except: resident.except.clone(),
                    };
                    town.renamed(&resident.name, &is).await
                }
                false => std::future::pending().await,
            }
        };
        tokio::select! {
            _ = living => {}
            _ = telling => {}
            _ = named => {}
        }
    }
}

/// Tell a resident what it hears, by `to`, until `to` fails or the town ends: the present of its
/// kinds, `now`, then each word since it came. What `to` cannot say is passed over, or, wanted of
/// it, its error; another's error told last marks what it says now as answering
/// (`Town::telling`).
pub async fn tell(
    town: &Town,
    resident: &Resident,
    mut words: Listening,
    mut to: impl AsyncFnMut(&Word, Option<&str>) -> std::io::Result<bool>,
) {
    // however it ends, it is told no more
    struct Telling<'a>(&'a Town, &'a Resident);
    impl Drop for Telling<'_> {
        fn drop(&mut self) {
            self.0.telling(self.1.id, None);
        }
    }
    let _telling = Telling(town, resident);
    if words.told() {
        town.befell(resident, "may have missed words before it was told".into(), false);
    }
    for w in resident.present(town).into_iter().chain([Word::now()]) {
        if to(&w, None).await.is_err() {
            return;
        }
    }
    while let Some((w, is_for)) = words.next().await {
        let by = match is_for {
            To::It => None,
            To::One(name) => Some(name),
        };
        // who is told it: itself, or the one it speaks for that the name is
        let spoken = by.as_ref().and_then(|name| {
            town.spoken_by(resident.id).iter().find(|(n, _)| n == name).map(|(_, r)| r.id)
        });
        town.telling(spoken.unwrap_or(resident.id), Some(&w));
        let (wanted, kind) = (by.is_none() && resident.wanted(town, &w), w.kind.clone());
        match to(&resident.placed(w), by.as_deref()).await {
            Ok(true) => {}
            Ok(false) if wanted => {
                town.error(resident, Some(&kind), "cannot be said in its language".into())
            }
            Ok(false) => {}
            Err(_) => return,
        }
    }
}

/// A resident's voice: what it says, as it comes, a line at a time, or a unit of a language at a
/// time, before it is words (`Speech`). A line is text whatever bytes it contains: what is not
/// UTF-8 reads as �, so one bad line is a line like any other, and what comes after it is heard.
/// Safe to wait on in a `select!`: what is half read is kept for the next wait.
pub struct Voice<R> {
    from: tokio::io::BufReader<R>,
    held: Vec<u8>,
    ended: bool,
    /// how many times a unit has been read, for a test to see the work bounded
    #[cfg(test)]
    tries: usize,
}

impl<R: AsyncRead + Unpin> Voice<R> {
    pub fn new(r: R) -> Self {
        let from = tokio::io::BufReader::new(r);
        #[cfg(test)]
        return Voice { from, held: Vec::new(), ended: false, tries: 0 };
        #[cfg(not(test))]
        Voice { from, held: Vec::new(), ended: false }
    }

    /// The next line, without its end; nothing once the stream has ended or failed. A line held
    /// already, read as a unit was looked for, is the next.
    pub async fn next(&mut self) -> Option<String> {
        // how far is looked at for a line's end: each byte once, however long the line
        let mut seen = 0;
        loop {
            let end = self.held[seen..].iter().position(|&c| c == b'\n').map(|n| seen + n);
            seen = self.held.len();
            match end {
                Some(n) => return Some(self.line(n + 1)),
                None if self.ended => {
                    return (!self.held.is_empty()).then(|| self.line(self.held.len()))
                }
                None => {}
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

    /// what comes, held, until the end
    async fn held(&mut self) {
        while !self.ended {
            self.more().await;
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
    ///   is held has grown by half since it was last read, or no more has come (`wait`).
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
            if self.ended && self.held.is_empty() {
                return None;
            }
            #[cfg(test)]
            {
                self.tries += 1;
            }
            match language.read(&self.held, self.ended, languages) {
                Read::Got(p, n) if n > 0 => return Some(Ok((p, n))),
                Read::More(need) => {
                    let grown = tried + tried / 2;
                    tried = self.held.len();
                    self.wait(need, grown).await
                }
                // nothing held is not yet anything: what comes may be of it
                _ if self.held.is_empty() => self.more().await,
                // not of it, or of it but empty: let go
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
                Read::More(_) => return at,
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
    /// grammar says it needs. Then on to `grown`, while more has come already: a grammar that
    /// cannot say what it needs says a byte more, and read again for each piece would read a unit
    /// again from its start each time a little comes. What has come is read once no more has.
    async fn wait(&mut self, need: usize, grown: usize) {
        let need = need.max(self.held.len() + 1);
        while self.held.len() < need && !self.ended {
            self.more().await;
        }
        while self.held.len() < grown && !self.ended && self.come().await {}
    }

    /// what has come already, held, if any has: all else that waits goes first
    async fn come(&mut self) -> bool {
        tokio::task::yield_now().await;
        tokio::select! {
            biased;
            _ = self.more() => true,
            _ = std::future::ready(()) => false,
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

    /// The next word said, and by whom: `from`, or the one it speaks for that the word names `by`.
    /// Nothing once it has stopped speaking.
    pub async fn said(&mut self, town: &Town, from: &Resident) -> Option<(Word, Resident)> {
        let at = from.at.as_deref().unwrap_or("");
        loop {
            // none speaks it now: what it says waits for it
            let language = match town.languages.get(&self.language) {
                Ok(language) => language,
                Err(_) => {
                    let waits = async |message: &str| _ = town.notice(from, message.into());
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
/// `a/b`, and `at` itself for none; but `now` is `now` wherever it is said, as it is told. One
/// that names no kind, `a//b` or `../x`, is no word.
fn placed(at: &str, mut w: Word) -> Result<Word, String> {
    if w.is_now() {
        return Ok(w);
    }
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
/// language cannot say is not written, and is `false`.
pub async fn say(
    to: &mut (impl AsyncWrite + Unpin),
    word: &Word,
    by: Option<&str>,
    language: &Language,
    languages: &Languages,
    most: Option<usize>,
) -> std::io::Result<bool> {
    match bytes(word, by, language, languages) {
        Some(b) if most.is_none_or(|most| b.len() <= most) => to.write_all(&b).await.map(|_| true),
        _ => Ok(false),
    }
}

/// A word in `language`, as bytes, and `by`, the one spoken for it is told for, beside it, as a
/// word said for one is: a line of JSON, or a unit of its language; nothing if the language cannot
/// say it.
pub fn bytes(
    word: &Word,
    by: Option<&str>,
    language: &Language,
    languages: &Languages,
) -> Option<Vec<u8>> {
    match (language, by) {
        (Language::Json, None) => Some((line(word) + "\n").into_bytes()),
        (Language::Json, Some(by)) => {
            let mut line = line(word);
            line.pop();
            let by = serde_json::to_string(by).unwrap_or_default();
            Some(format!("{line},\"by\":{by}}}\n").into_bytes())
        }
        (_, by) => {
            let mut p = parts(word);
            if let Some(by) = by {
                p.set("by", by);
            }
            language.bytes(&p, languages)
        }
    }
}

/// a word as parts: `kind`, `tense`, `body`, and `when` once it is said
pub fn parts(w: &Word) -> Parts {
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
/// - **what it contains**: `WORD` and nothing else, as `of` reads parts; and, said by a resident,
///   `by` (`spoken`).
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

/// A line a resident says, or is told, as a word, and `by`, the one spoken for that says it or is
/// told it, if it names one.
pub fn said(line: &str) -> Result<(Word, Option<String>), String> {
    let l: Line = crate::parts::json(line.as_bytes()).map_err(|e| e.to_string())?;
    let tense = match l.tense {
        None => Tense::Present,
        Some(t) => Tense::named(&t).ok_or("a tense is past, present or future")?,
    };
    let body = l.body.map_or_else(Body::none, |b| Body::said(b.get().as_bytes()));
    let by = l.by.map(Cow::into_owned);
    Ok((Word { kind: l.kind.into_owned(), tense, body, when: l.when, by: 0 }, by))
}

/// A word in town's own language, whole, as the log keeps it: a line of JSON. The one
/// place town's own words become bytes; `owns` reads them back.
pub fn own(w: &Word) -> Vec<u8> {
    let mut bytes = line(w).into_bytes();
    bytes.push(b'\n');
    bytes
}

/// What bytes in town's own language contain, in order: each unit as it was said, and the word it
/// is, if it is one. One cut short, or that is no word, is none, and those around it stand.
pub fn owns(bytes: &[u8]) -> Vec<(&[u8], Option<Word>)> {
    let unit = |unit: &[u8]| {
        let text = String::from_utf8_lossy(unit);
        read(text.trim_end_matches('\n')).ok()
    };
    bytes.split_inclusive(|&b| b == b'\n').map(|u| (u, unit(u))).collect()
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
        let to = async |w: &Word, _: Option<&str>| {
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

    // one come by a name is not told what passes through the name, as it comes or after: so what
    // came from a town, by any way, is not told back to it
    #[test]
    fn one_come_by_a_name_is_not_told_what_passes_through_it() {
        let dir = scratch();
        let town = Town::with_log(dir.log());
        town.talk(Word::new("theme", Tense::Present, Body::of("dark")));
        town.talk(Word::new("luke.example/old", Tense::Present, Body::of(1i64)));
        town.talk(Word::new("york.example/luke.example/x", Tense::Present, Body::of(1i64)));
        let who = town.who("luke.example", true);
        let luke = Resident { kinds: vec!["/".into()], visitor: true, ..who };
        let told: Vec<String> = luke.present(&town).into_iter().map(|w| w.kind).collect();
        assert_eq!(told, ["theme"], "nothing through luke.example");
        let said = |kind: &str| Word::new(kind, Tense::Past, Body::none());
        for part in ["luke.example.old/x", "x/luke", "luke"] {
            assert!(luke.hears(&town, &said(part)), "through it only in part: {part}");
        }
        for whole in ["luke.example", "luke.example/x", "a/luke.example/x", "a/luke.example"] {
            assert!(!luke.hears(&town, &said(whole)), "through it: {whole}");
        }
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
            let waits = 4;
            let resident = Resident { kinds: vec!["theme".into()], waits: Some(waits), ..who };
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
            for _ in 0..waits {
                town.talk(Word::new("theme", Tense::Past, Body::none()));
            }
            town.talk(Word::new("theme", Tense::Present, Body::of("light")));
            assert!(errors.try_recv().is_err(), "not said to fall behind before it is told");
            let mut heard = Vec::new();
            let to = async |w: &Word, _: Option<&str>| {
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

    /// what a speaker is told, as `(kind, body, by)`, until a past `s` of `end`
    async fn told_for(
        town: &Town,
        s: &Resident,
        words: Listening,
        mut then: impl FnMut(),
    ) -> Vec<(String, Body, Option<String>)> {
        let mut heard = Vec::new();
        let to = async |w: &Word, by: Option<&str>| {
            if w.body == Body::of("end") {
                return Err(std::io::ErrorKind::BrokenPipe.into());
            }
            if w.is_now() && by.is_none() {
                then();
            }
            heard.push((w.kind.clone(), w.body.clone(), by.map(String::from)));
            Ok(true)
        };
        tell(town, s, words, to).await;
        heard
    }

    // a speaker is told each word once for each that hears it: bare, for itself; `by` each it
    // speaks for, however deep, by its name from its place. One it comes to speak for is greeted
    // with its own present, then `now`, `by` it; one gone is told no more
    #[test]
    fn a_speaker_is_told_each_word_for_each_it_speaks_for() {
        run(async {
            let dir = scratch();
            let town = Town::with_log(dir.log());
            let s = Resident { kinds: vec!["s".into()], ..town.who("s", false) };
            let _s = town.come(&s).unwrap();
            town.talk(Word::new("s", Tense::Present, Body::of("mine")));
            town.talk(Word::new("x", Tense::Present, Body::of("one")));
            let it = |kinds: serde_json::Value| Body::of(serde_json::json!({ "kinds": kinds }));
            let say = |name: &str, body: Body, by: &Resident| {
                town.said(Word::new(format!("town/residents/{name}"), Tense::Present, body), by)
            };
            let past = |kind: &str, body: &str| {
                town.talk(Word::new(kind, Tense::Past, Body::of(body)));
            };
            let then = || {
                say("a", it(serde_json::json!(["x"])), &s);
                let a = town.spoken_for(&s, "a").unwrap();
                say("b", it(serde_json::json!(["x", "y"])), &a);
                past("x", "two");
                past("y", "why");
                say("a", Body::none(), &s);
                past("x", "three");
                past("s", "end");
            };
            let heard = told_for(&town, &s, s.listening(&town), then).await;
            let by = |name: &str| Some(name.to_string());
            let want = [
                ("s", Body::of("mine"), None),
                ("", Body::none(), None),
                ("x", Body::of("one"), by("a")),
                ("", Body::none(), by("a")),
                ("x", Body::of("one"), by("a/b")),
                ("", Body::none(), by("a/b")),
                ("x", Body::of("two"), by("a")),
                ("x", Body::of("two"), by("a/b")),
                ("y", Body::of("why"), by("a/b")),
            ];
            let want: Vec<_> = want.into_iter().map(|(k, b, by)| (k.to_string(), b, by)).collect();
            assert_eq!(heard, want, "each for each that hears it, and none once gone");
        });
    }

    // one a speaker comes to speak for before it is told is greeted all the same, after the
    // speaker's own present
    #[test]
    fn a_speaker_not_yet_told_is_greeted_for_those_it_speaks_for() {
        run(async {
            let dir = scratch();
            let town = Town::with_log(dir.log());
            let s = Resident { kinds: vec!["s".into()], ..town.who("s", false) };
            let _s = town.come(&s).unwrap();
            town.talk(Word::new("x", Tense::Present, Body::of("one")));
            let words = s.listening(&town);
            let body = Body::of(serde_json::json!({ "kinds": ["x"] }));
            town.said(Word::new("town/residents/a", Tense::Present, body), &s);
            let end = || town.talk(Word::new("s", Tense::Past, Body::of("end")));
            let heard = told_for(&town, &s, words, end).await;
            let a = Some("a".to_string());
            let want = [
                ("".into(), Body::none(), None),
                ("x".into(), Body::of("one"), a.clone()),
                ("".into(), Body::none(), a),
            ];
            assert_eq!(heard, want, "its own present, then the greeting");
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
            assert_eq!(speech.said(&town, &me).await.unwrap().0.kind, "a");
            let b = speech.said(&town, &me).await.unwrap().0;
            assert_eq!(b.kind, "b", "past the blank and the bad");
            assert!(speech.said(&town, &me).await.is_none());
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
        let at_root = Parts::map([("tense", "past".into()), ("body", Parts::from("x"))]);
        assert!(word_at("", &at_root).is_err(), "at the root, a kind");
        assert!(word_at("", &Parts::map([("tense", "past")])).unwrap().is_now(), "but `now`");
        // at a join, `""` is its kind; but a past word there with no body is `now`, as it is told
        let at_box =
            |p: Parts| word_at("box", &Parts::map([("tense", "past".into()), ("body", p)]));
        assert_eq!(at_box(Parts::from("x")).unwrap().kind, "box");
        assert!(at_box(Parts::None).unwrap().is_now(), "`now`, wherever it is said");
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

    // a line is whole, however long, and the next is heard
    #[test]
    fn a_line_is_whole() {
        run(async {
            let mut said = vec![b'a'; 1 << 20];
            said.extend_from_slice(b"\nnext\n");
            let mut voice = Voice::new(&said[..]);
            assert_eq!(voice.next().await.map(|l| l.len()), Some(1 << 20));
            assert_eq!(voice.next().await.as_deref(), Some("next"));
            assert_eq!(voice.next().await, None);
        });
    }

    // a unit that comes in many pieces is read again only as what is held grows by half, not for
    // each piece that has come, so its reads cost what a few of it whole would; and one that comes
    // and stops is read, though less came than was waited for
    #[test]
    fn a_unit_is_read_again_as_it_grows() {
        run(async {
            let languages = Languages::default();
            let line = crate::language::Grammar::of(&Parts::map([("upto", "\n")])).unwrap();
            languages.say("line", line);
            let (mut to, from) = tokio::io::duplex(256);
            let said = 64 * 1024 - 1;
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

    // in a language not JSON, a unit that is no word is said as the bytes it was read from, so
    // the one who said it knows it
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
            assert_eq!(speech.said(&town, &Resident::town()).await.unwrap().0.kind, "c");
            let e = heard.try_recv().expect("the unit that is no word is an error");
            assert_eq!(e.body.value()["said"], "a//b");
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
