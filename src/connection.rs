//! A connection: whatever town hears from and speaks to, a program, a client at the square, a
//! joined program, a socket. What it says is read here as words, and a word is written here as
//! what it hears, in the language it speaks: JSON a line each, unless it says otherwise; a grammar
//! as far as it reads. Here, and only here, bytes become words and words bytes.

use std::borrow::Cow;
use std::rc::Rc;

use serde::Deserialize;
use serde_json::value::RawValue;
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};

use crate::body::Body;
use crate::language::{Language, Languages, Read};
use crate::parts::Parts;
use crate::town::Town;
use crate::word::{Tense, Word};

/// The most a unit may take: a header that says more is not of its language, so none can make town
/// hold without end.
const MOST: usize = 16 * 1024 * 1024;

/// What a connection says, as it comes: a line at a time, or a unit of a language at a time. A
/// line is text whatever bytes it holds: what is not UTF-8 reads as �, so one bad line is a line
/// like any other, and what comes after it is heard. Safe to wait on in a `select!`: what is half
/// read is kept for the next wait.
pub struct Heard<R> {
    from: tokio::io::BufReader<R>,
    held: Vec<u8>,
    ended: bool,
}

impl<R: AsyncRead + Unpin> Heard<R> {
    pub fn new(r: R) -> Self {
        Heard { from: tokio::io::BufReader::new(r), held: Vec::new(), ended: false }
    }

    /// the next line, without its end; nothing once the stream has ended or failed
    pub async fn next(&mut self) -> Option<String> {
        use tokio::io::AsyncBufReadExt;
        let more = matches!(self.from.read_until(b'\n', &mut self.held).await, Ok(n) if n > 0);
        if !more && self.held.is_empty() {
            return None;
        }
        let end = [&b"\r\n"[..], b"\n"].into_iter().find(|e| self.held.ends_with(e));
        self.held.truncate(self.held.len() - end.map_or(0, |e| e.len()));
        let line = String::from_utf8_lossy(&self.held).into_owned();
        self.held.clear();
        Some(line)
    }

    /// The next unit said in `language`: its parts, or the bytes that are not of it, so what comes
    /// after them is heard: as far as the end of their line, or, with no line's end among them, as
    /// far as it is of the language again, since a binary language has no lines. Nothing once the
    /// stream has ended. A unit is read again only once as many bytes as it needs have come, and
    /// none takes more than `MOST`.
    pub async fn unit(
        &mut self,
        language: &Language,
        languages: &Languages,
    ) -> Option<Result<Parts, Vec<u8>>> {
        loop {
            // the end, but for blank lines in JSON, which are nothing
            let blank = matches!(language, Language::Json) && self.held.trim_ascii().is_empty();
            if self.ended && (self.held.is_empty() || blank) {
                return None;
            }
            match language.read(&self.held, self.ended, languages) {
                Read::Got(p, n) if n > 0 => {
                    self.held.drain(..n);
                    return Some(Ok(p));
                }
                Read::More(need) if need <= MOST => self.wait(need).await,
                // nothing held is not yet anything: what comes may be of it
                _ if self.held.is_empty() => self.more().await,
                // not of it, of it but empty, or more than a unit may take: let go as far as the
                // end of its line, or, with no line's end held, as a binary language has none, as
                // far as it is of it again
                _ => {
                    let again =
                        |k: &usize| match language.read(&self.held[*k..], self.ended, languages) {
                            Read::Got(_, n) => n > 0,
                            Read::More(need) => need <= MOST,
                            Read::No => false,
                        };
                    let n = match self.held.iter().position(|&c| c == b'\n') {
                        Some(n) => n + 1,
                        None => (1..self.held.len()).find(again).unwrap_or(self.held.len()),
                    };
                    return Some(Err(self.held.drain(..n).collect()));
                }
            }
        }
    }

    /// The next unit said in `language`, as `take` makes it, if what is held begins with one that
    /// `take` takes; if it does not, nothing is taken, so it is held for another language to read.
    /// `None` once the stream has ended.
    pub async fn unit_if<T>(
        &mut self,
        language: &Language,
        languages: &Languages,
        take: impl Fn(&Parts) -> Option<T>,
    ) -> Option<Option<T>> {
        loop {
            if self.ended && self.held.trim_ascii().is_empty() {
                return None;
            }
            match language.read(&self.held, self.ended, languages) {
                Read::Got(p, n) if n > 0 => {
                    let taken = take(&p);
                    if taken.is_some() {
                        self.held.drain(..n);
                    }
                    return Some(taken);
                }
                Read::More(need) if need <= MOST => self.wait(need).await,
                _ if self.held.is_empty() => self.more().await,
                _ => return Some(None),
            }
        }
    }

    /// as many bytes held as a unit needs, or the end: more is more, a byte at least, whatever a
    /// grammar says it needs
    async fn wait(&mut self, need: usize) {
        let need = need.max(self.held.len() + 1);
        while self.held.len() < need && !self.ended {
            self.more().await;
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

/// What a connection says, as words in its language: in JSON, a blank line is nothing; and what is
/// no word is an `error` from whoever said it. Safe to wait on in a `select!`, as what is heard is.
pub struct Speech<R>(Heard<R>, Rc<Language>);

impl<R: AsyncRead + Unpin> Speech<R> {
    /// what is `heard`, in `language`, after what has been heard already
    pub fn new(heard: Heard<R>, language: Rc<Language>) -> Self {
        Speech(heard, language)
    }

    /// the next word said, by `from`; nothing once it has stopped speaking
    pub async fn next(&mut self, town: &Town, from: &str) -> Option<Word> {
        loop {
            if let Language::Json = *self.1 {
                if let Some(word) = word(town, from, &self.0.next().await?) {
                    return Some(word);
                }
                continue;
            }
            let (said, why) = match self.0.unit(&self.1, &town.languages).await? {
                Ok(p) => match of(&p) {
                    Ok(word) => return Some(word),
                    Err(e) => (serde_json::to_string(&p).unwrap_or_default(), e),
                },
                Err(bytes) => {
                    (String::from_utf8_lossy(&bytes).into_owned(), "not of its language".into())
                }
            };
            town.misheard(from, said.trim_end_matches('\n'), format!("not a word: {why}"));
        }
    }
}

/// A line of JSON said by `from`, as a word: nothing if it is blank, and an `error` if it is no
/// word. The one place a line becomes a word.
pub fn word(town: &Town, from: &str, line: &str) -> Option<Word> {
    if line.trim().is_empty() {
        return None;
    }
    read(line).map_err(|e| town.misheard(from, line, format!("not a word: {e}"))).ok()
}

/// Write a word to whoever hears it in `language`: the one place a word becomes bytes. A word the
/// language cannot hold is not written, and is `false`.
pub async fn say(
    to: &mut (impl AsyncWrite + Unpin),
    word: &Word,
    language: &Language,
    languages: &Languages,
) -> std::io::Result<bool> {
    let bytes = match language {
        Language::Json => line(word) + "\n",
        _ => match language.bytes(&parts(word), languages) {
            Some(b) => return to.write_all(&b).await.map(|_| true),
            None => return Ok(false),
        },
    };
    to.write_all(bytes.as_bytes()).await.map(|_| true)
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

/// parts as a word: a `kind`, and a `tense` if it is not present
pub fn of(p: &Parts) -> Result<Word, String> {
    let kind = p.get("kind").and_then(Parts::text).ok_or("a word has a kind")?;
    let tense = match p.get("tense") {
        None => Tense::Present,
        Some(t) => t.text().and_then(Tense::named).ok_or("a tense is past, present or future")?,
    };
    let mut w = Word::new(kind, tense, Body::of(p.get("body").cloned().unwrap_or_default()));
    w.when = p.get("when").and_then(Parts::number).map_or(0, |n| n as u64);
    Ok(w)
}

/// A word as a line of JSON, as the log says it, and the square and programs unless they speak
/// another: `{ kind, tense, body, when }`, a tense present if it is not said, and `at` read as
/// `when`. Its body is kept as the bytes it came in, read only if a part is asked for.
#[derive(Deserialize)]
struct Line<'a> {
    #[serde(borrow)]
    kind: Cow<'a, str>,
    #[serde(borrow, default)]
    tense: Option<Cow<'a, str>>,
    #[serde(borrow, default)]
    body: Option<&'a RawValue>,
    #[serde(default, alias = "at")]
    when: u64,
}

/// a line, as a word
pub fn read(line: &str) -> Result<Word, String> {
    let l: Line = serde_json::from_str(line).map_err(|e| e.to_string())?;
    let tense = match l.tense {
        None => Tense::Present,
        Some(t) => Tense::named(&t).ok_or("a tense is past, present or future")?,
    };
    let body = l.body.map_or_else(Body::none, |b| Body::said(b.get().as_bytes()));
    Ok(Word { kind: l.kind.into_owned(), tense, body, when: l.when })
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
    use crate::body::Body;

    fn run(f: impl std::future::Future<Output = ()>) {
        tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(f)
    }

    // in JSON a blank line is nothing, between words and after the last
    #[test]
    fn a_blank_line_is_nothing() {
        run(async {
            let mut heard = Heard::new(&b"\n{\"a\":1}\n\n \n"[..]);
            let languages = Languages::default();
            let a = heard.unit(&Language::Json, &languages).await;
            assert_eq!(a, Some(Ok(Parts::map([("a", Parts::from(1i64))]))));
            assert_eq!(heard.unit(&Language::Json, &languages).await, None);
        });
    }

    // a language that reads nothing of nothing waits for what comes, rather than calling nothing
    // astray again and again
    #[test]
    fn nothing_held_is_waited_on() {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        tokio::task::LocalSet::new().block_on(&rt, async {
            let languages = Languages::default();
            let dangling = crate::language::Grammar::of(&Parts::map([("ref", "none")])).unwrap();
            languages.say("dangling", dangling);
            let dangling = languages.get("dangling").unwrap();
            let (mut to, from) = tokio::io::duplex(64);
            tokio::task::spawn_local(async move {
                tokio::task::yield_now().await;
                to.write_all(b"x").await.unwrap();
            });
            let mut heard = Heard::new(from);
            assert_eq!(heard.unit(&dangling, &languages).await, Some(Err(b"x".to_vec())));
            assert_eq!(heard.unit(&dangling, &languages).await, None);
        });
    }

    // a line that is not UTF-8 is a line like any other, and those after it are heard; a last one
    // without its end is heard too
    #[test]
    fn lines_are_read_whatever_they_hold() {
        run(async {
            let mut heard = Heard::new(&b"one\r\nbad \xff here\n{\"kind\":\"a\"}\nlast"[..]);
            let mut said = Vec::new();
            while let Some(line) = heard.next().await {
                said.push(line);
            }
            assert_eq!(said, ["one", "bad \u{fffd} here", "{\"kind\":\"a\"}", "last"]);
        });
    }

    // what is said is heard as words: a blank line is nothing, one that is no word is an `error`
    // from whoever said it, and what comes after either is heard
    #[test]
    fn speech_is_words() {
        run(async {
            let town = Town::with_log(std::env::temp_dir().join("town-speech-never-written.log"));
            let mut heard = town.words.subscribe();
            let said = &b"{\"kind\":\"a\"}\n\n{\"body\":1}\n{\"kind\":\"b\"}\n"[..];
            let mut speech = Speech::new(Heard::new(said), Language::json());
            assert_eq!(speech.next(&town, "me").await.unwrap().kind, "a");
            assert_eq!(
                speech.next(&town, "me").await.unwrap().kind,
                "b",
                "past the blank and the bad"
            );
            assert!(speech.next(&town, "me").await.is_none());
            let e = heard.try_recv().expect("the bad line is an error");
            assert_eq!((e.kind.as_str(), e.body.value()["from"].as_str()), ("error", Some("me")));
            assert!(e.body.value()["message"].as_str().unwrap().starts_with("not a word"));
            assert_eq!(e.body.value()["said"], "{\"body\":1}", "what was said, to know it by");
            assert!(heard.try_recv().is_err(), "and the blank line is nothing");
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

    // a line read is written as it came, its body unread; a tense not said is present, `at` is
    // `when`, and a tense that is none is no word
    #[test]
    fn a_line_goes_on_as_it_came() {
        let said = r#"{"kind":"a","tense":"past","body":{ "b" : [1,  2] },"when":5}"#;
        let w = read(said).unwrap();
        assert_eq!((w.tense, w.when), (Tense::Past, 5));
        assert_eq!(line(&w), said);
        let w = read(r#"{"kind":"\u0061","body":{"who":"me"},"at":7}"#).unwrap();
        assert_eq!((w.kind.as_str(), w.tense, w.when), ("a", Tense::Present, 7));
        assert_eq!(w.body.part("who").and_then(crate::parts::Parts::text), Some("me"));
        assert!(read(r#"{"kind":"a","tense":"soon"}"#).is_err());
    }
}
