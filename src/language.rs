//! A language: how bytes are read as a body's parts, and its parts written as bytes. One grammar,
//! run both ways, so a language is written once. A town's languages are said by its residents; JSON
//! is the one town has before any resident speaks, for its log and its square.
//!
//! A grammar reads a stream: each piece reads, or does not, or needs more bytes than there are yet,
//! so a connection waits for the rest of a word rather than taking half of one for none.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use serde::Deserialize;

use crate::parts::Parts;

pub enum Language {
    Json,
    Grammar(Grammar),
}

/// What the bytes so far come to, in a language.
#[derive(Debug, PartialEq)]
pub enum Read {
    /// parts, and how many bytes they took
    Got(Parts, usize),
    /// bytes that are not of it
    No,
    /// bytes that may be, once more come: so many in all, at least
    More(usize),
}

impl Language {
    /// JSON, the same one wherever it is asked for
    pub fn json() -> Rc<Language> {
        thread_local!(static JSON: Rc<Language> = Rc::new(Language::Json));
        JSON.with(Rc::clone)
    }

    /// The first of what `bytes` hold, in this language: JSON a line at a time, a grammar as far as
    /// it reads. `ended` once no more will come.
    pub fn read(&self, bytes: &[u8], ended: bool, languages: &Languages) -> Read {
        match self {
            Language::Json => {
                // a blank line is nothing, passed over
                let blank = bytes.iter().take_while(|c| c.is_ascii_whitespace()).count();
                let rest = &bytes[blank..];
                let (line, took) = match rest.iter().position(|&c| c == b'\n') {
                    Some(n) => (&rest[..n], blank + n + 1),
                    None if ended && !rest.is_empty() => (rest, bytes.len()),
                    None if ended => return Read::No,
                    None => return Read::More(bytes.len() + 1),
                };
                match serde_json::from_slice(line) {
                    Ok(p) => Read::Got(p, took),
                    Err(_) => Read::No,
                }
            }
            Language::Grammar(g) => g.read_from(bytes, ended, languages),
        }
    }

    /// all of `bytes`, as parts; nothing if they are not of this language
    pub fn parts(&self, bytes: &[u8], languages: &Languages) -> Option<Parts> {
        match self {
            Language::Json => serde_json::from_slice(bytes).ok(),
            Language::Grammar(g) => match g.read_from(bytes, true, languages) {
                Read::Got(p, n) if n == bytes.len() => Some(p),
                _ => None,
            },
        }
    }

    /// the parts, as bytes; nothing if they cannot be written in this language
    pub fn bytes(&self, parts: &Parts, languages: &Languages) -> Option<Vec<u8>> {
        match self {
            // a value on a line, as it is read
            Language::Json => serde_json::to_vec(parts).ok().map(|mut b| {
                b.push(b'\n');
                b
            }),
            Language::Grammar(g) => {
                let mut w = Writer { out: Vec::new(), languages, deep: 0, writing: Vec::new() };
                g.write(parts, true, &mut w).then_some(w.out)
            }
        }
    }
}

/// JSON's own values, as the parts they hold
impl From<serde_json::Value> for Parts {
    fn from(v: serde_json::Value) -> Parts {
        Parts::deserialize(v).unwrap_or_default()
    }
}

/// A town's languages, by name: those said since it last opened, and JSON.
#[derive(Default)]
pub struct Languages(RefCell<HashMap<String, Rc<Language>>>);

impl Languages {
    /// a language said: a later one of the same name takes its place
    pub fn say(&self, name: &str, grammar: Grammar) {
        self.0.borrow_mut().insert(name.to_string(), Rc::new(Language::Grammar(grammar)));
    }

    /// as the town opens again, its residents say their languages again
    pub fn clear(&self) {
        self.0.borrow_mut().clear();
    }

    pub fn get(&self, name: &str) -> Result<Rc<Language>, String> {
        match name {
            "json" => Ok(Language::json()),
            _ => self.0.borrow().get(name).cloned().ok_or(format!("no language is called {name}")),
        }
    }

    /// bytes, as the parts the language `name` reads in them
    pub fn parts(&self, name: &str, bytes: &[u8]) -> Result<Parts, String> {
        self.get(name)?.parts(bytes, self).ok_or(format!("not {name}"))
    }

    /// parts, as the bytes the language `name` writes of them
    pub fn bytes(&self, name: &str, parts: &Parts) -> Result<Vec<u8>, String> {
        self.get(name)?.bytes(parts, self).ok_or(format!("cannot be written as {name}"))
    }
}

/// How deep a language may name another, or parts hold parts: either may go on without end.
pub const DEEP: usize = 128;

#[derive(Clone, Debug, PartialEq)]
pub enum Grammar {
    /// these bytes; named, they are kept
    Lit(Vec<u8>),
    /// text up to these bytes, which are passed over; or, once the bytes end, up to the end
    Upto(Vec<u8>),
    /// text of one byte or more, none of them these
    But(Vec<u8>),
    /// each in turn: the parts named within it; with none, what its one other piece reads
    Seq(Vec<Grammar>, Shape),
    /// what is read, by this name
    Part(String, Box<Grammar>),
    /// the first that reads
    Choice(Vec<Grammar>),
    /// as many as read, in order; or as many as the count says
    Many(Box<Grammar>, Option<Count>),
    /// as many as read, each the parts `key` and `value`; or as many as the count says
    Map(Box<Grammar>, Option<Count>),
    /// nothing, where this would not read
    Unless(Box<Grammar>),
    /// a whole number
    Int(Int),
    /// a number of 2, 4 or 8 bytes, as IEEE 754 holds it
    Float(usize, bool),
    /// text of as many bytes as the count says
    Text(Count),
    /// bytes, as many as the count says; written, text is its bytes
    Raw(Count),
    /// what is read, as this part
    Be(Box<Grammar>, Parts),
    /// the language of this name
    Ref(String),
    /// the text, or the bytes, that this reads in pieces, as one
    Whole(Box<Grammar>),
}

/// What a seq's pieces come to, worked out once, as it is made.
#[derive(Clone, Debug, PartialEq)]
pub struct Shape {
    /// the parts that are counts of others, and not kept
    counts: Vec<String>,
    /// whether it keeps parts by name: some piece is neither a mark nor a count
    kept: bool,
    /// whether it is its one other piece: no part named but counts, and one piece neither a mark
    /// nor a count
    lone: bool,
}

impl Shape {
    fn of(items: &[Grammar]) -> Shape {
        let counts: Vec<String> = items.iter().filter_map(counted_by).map(String::from).collect();
        let count = |g: &Grammar| matches!(g, Grammar::Part(n, _) if counts.contains(n));
        let mark = |g: &Grammar| matches!(g, Grammar::Lit(_) | Grammar::Unless(_));
        let named = items.iter().any(|g| matches!(g, Grammar::Part(..)) && !count(g));
        let others = items.iter().filter(|g| !mark(g) && !count(g)).count();
        Shape { kept: others > 0, lone: !named && others == 1, counts }
    }

    fn counts(&self, name: &str) -> bool {
        self.counts.iter().any(|c| c == name)
    }
}

/// How many: so many; as many as a grammar reads as a number; or as many as a part read before it,
/// in the same seq, says. That part is the count, and is not kept; written, it is the length of
/// what it counts.
#[derive(Clone, Debug, PartialEq)]
pub enum Count {
    Fixed(usize),
    Read(Box<Grammar>),
    Named(String),
}

/// A whole number of `size` bytes, its first byte the most or, `little`, the least: `from` to `to`
/// as the bytes hold it, and read as that less `base`; or, `negative`, as `base` less that.
#[derive(Clone, Debug, PartialEq)]
pub struct Int {
    size: usize,
    signed: bool,
    little: bool,
    from: i128,
    to: i128,
    base: i128,
    negative: bool,
}

/// what a piece of a grammar comes to, reading
enum R {
    Got(Parts),
    No,
    /// so many bytes in all, at least
    More(usize),
}

/// what a piece reads, or what it comes to if it does not
macro_rules! got {
    ($r:expr) => {
        match $r {
            R::Got(p) => p,
            other => return other,
        }
    };
}

struct Reader<'a> {
    b: &'a [u8],
    ended: bool,
    languages: &'a Languages,
    deep: usize,
    /// the whole numbers named so far in the seqs being read, for a count that names one
    known: Vec<(String, usize)>,
    /// the languages being read, and where: one named again where it is already being read reads
    /// nothing, so a language that begins with itself ends
    reading: Vec<(*const Grammar, usize)>,
}

struct Writer<'a> {
    out: Vec<u8>,
    languages: &'a Languages,
    deep: usize,
    /// the languages being written, and of what: one named again for the same part writes nothing
    writing: Vec<(*const Grammar, *const Parts)>,
}

impl Grammar {
    /// A grammar as parts: one of its pieces, by name, and what that piece takes. `{ upto = "\n"
    /// }`, `{ part = { "key", g } }`, `{ seq = { g, … } }`, `{ many = { g, n } }`, `{ int = { size
    /// = 2 } }`.
    pub fn of(p: &Parts) -> Result<Grammar, String> {
        let one = match p {
            Parts::Map(m) if m.len() == 1 => &m[0],
            _ => return Err(format!("a grammar is one of {}", PIECES.join(", "))),
        };
        let (piece, g) = (one.0.as_str(), &one.1);
        let bytes = || match g {
            Parts::Text(t) => Ok(t.as_bytes().to_vec()),
            Parts::Bytes(b) => Ok(b.clone()),
            _ => Err(format!("`{piece}` takes bytes")),
        };
        let each = || match g {
            Parts::Map(m) if m.is_empty() => Ok(Vec::new()), // Lua's empty table
            _ => {
                g.list().ok_or(format!("`{piece}` takes a list"))?.iter().map(Grammar::of).collect()
            }
        };
        // a grammar, or a grammar and a count
        let counted = || -> Result<(Box<Grammar>, Option<Count>), String> {
            match g.list() {
                Some([g]) => Ok((Box::new(Grammar::of(g)?), None)),
                Some([g, n]) => Ok((Box::new(Grammar::of(g)?), Some(Count::of(n)?))),
                Some(_) => Err(format!("`{piece}` takes a grammar, and perhaps a count")),
                None => Ok((Box::new(Grammar::of(g)?), None)),
            }
        };
        Ok(match piece {
            "lit" => Grammar::Lit(bytes()?),
            "upto" => Grammar::Upto(bytes()?),
            "but" => Grammar::But(bytes()?),
            "seq" => {
                let items = each()?;
                let shape = Shape::of(&items);
                Grammar::Seq(items, shape)
            }
            "choice" => Grammar::Choice(each()?),
            "many" => counted().map(|(g, n)| Grammar::Many(g, n))?,
            "map" => counted().map(|(g, n)| Grammar::Map(g, n))?,
            "unless" => Grammar::Unless(Box::new(Grammar::of(g)?)),
            "int" => Grammar::Int(Int::of(g)?),
            "float" => {
                let size = number(g, "size").unwrap_or(8.0) as usize;
                if ![2, 4, 8].contains(&size) {
                    return Err("a float is 2, 4 or 8 bytes".into());
                }
                Grammar::Float(size, little(g)?)
            }
            "text" => Grammar::Text(Count::of(g)?),
            "raw" => Grammar::Raw(Count::of(g)?),
            "be" => match g.list() {
                Some([g]) => Grammar::Be(Box::new(Grammar::of(g)?), Parts::None),
                Some([g, v]) => Grammar::Be(Box::new(Grammar::of(g)?), v.clone()),
                _ => return Err("`be` takes a grammar and a part".into()),
            },
            "ref" => Grammar::Ref(g.text().ok_or("`ref` takes a language's name")?.into()),
            "whole" => Grammar::Whole(Box::new(Grammar::of(g)?)),
            "part" => match g.list() {
                Some([name, g]) => {
                    let name = name.text().ok_or("a part's name is text")?;
                    Grammar::Part(name.to_string(), Box::new(Grammar::of(g)?))
                }
                _ => return Err("`part` takes a name and a grammar".into()),
            },
            _ => return Err(format!("a grammar has no `{piece}`")),
        })
    }

    /// The first of what `bytes` hold, as far as this grammar reads them.
    pub fn read_from(&self, bytes: &[u8], ended: bool, languages: &Languages) -> Read {
        let (mut r, mut at) = (
            Reader { b: bytes, ended, languages, deep: 0, known: Vec::new(), reading: Vec::new() },
            0,
        );
        match self.read(&mut r, &mut at) {
            R::Got(p) => Read::Got(p, at),
            R::More(n) if !ended => Read::More(n),
            _ => Read::No,
        }
    }

    /// What is read from `at`, which it moves past. A grammar that does not read leaves `at` for
    /// whoever tries next to put back.
    fn read(&self, r: &mut Reader, at: &mut usize) -> R {
        let rest = &r.b[*at..];
        // so many bytes, if there are so many; or more to wait for
        let (ended, start) = (r.ended, *at);
        let take = |n: usize| match rest.len() >= n {
            true => Ok(&rest[..n]),
            false if ended => Err(R::No),
            false => Err(R::More(start + n)),
        };
        // one byte more than there is, at least
        let another = R::More(r.b.len() + 1);
        match self {
            Grammar::Lit(s) => {
                if rest.starts_with(s) {
                    *at += s.len();
                    return R::Got(leaf(s));
                }
                match !r.ended && s.starts_with(rest) {
                    true => R::More(*at + s.len()),
                    false => R::No,
                }
            }
            Grammar::Upto(s) => match find(rest, s) {
                Some(n) => {
                    *at += n + s.len();
                    R::Got(text(&rest[..n]))
                }
                None if !r.ended => another,
                None if !rest.is_empty() => {
                    *at += rest.len();
                    R::Got(text(rest))
                }
                None => R::No,
            },
            Grammar::But(s) => {
                let n = rest.iter().take_while(|c| !s.contains(c)).count();
                if n == rest.len() && !r.ended {
                    return another;
                }
                if n == 0 {
                    return R::No;
                }
                *at += n;
                R::Got(text(&rest[..n]))
            }
            Grammar::Seq(items, shape) => {
                let (mut parts, mut lone, known) = (None, Vec::new(), r.known.len());
                for item in items {
                    let v = match item.read(r, at) {
                        R::Got(v) => v,
                        other => {
                            r.known.truncate(known);
                            return other;
                        }
                    };
                    // a whole number named is known, for a count that names it
                    if let Grammar::Part(name, _) = item {
                        if let Some(n) =
                            v.get(name).and_then(whole).and_then(|n| usize::try_from(n).ok())
                        {
                            r.known.push((name.clone(), n));
                        }
                    }
                    match (item, v) {
                        (Grammar::Lit(_) | Grammar::Unless(_), _) => {}
                        (_, Parts::Map(p)) => {
                            let parts = parts.get_or_insert(Parts::Map(Vec::new()));
                            p.into_iter().for_each(|(n, v)| parts.set(n, v));
                        }
                        (_, Parts::None) => {}
                        (_, v) => lone.push(v),
                    }
                }
                r.known.truncate(known);
                // a part that is a count is what it counts, and not kept: parts of none but counts
                // are no parts
                if let Some(p) = parts.as_mut() {
                    shape.counts.iter().for_each(|n| p.remove(n));
                }
                R::Got(match (parts, lone.len()) {
                    (Some(Parts::Map(m)), 1) if m.is_empty() => lone.remove(0),
                    (Some(parts), _) if shape.kept => parts,
                    (_, 1) => lone.remove(0),
                    _ => Parts::None,
                })
            }
            Grammar::Part(name, g) => R::Got(Parts::map([(name.clone(), got!(g.read(r, at)))])),
            Grammar::Choice(gs) => {
                for g in gs {
                    let from = *at;
                    match g.read(r, at) {
                        R::No => *at = from,
                        read => return read,
                    }
                }
                R::No
            }
            Grammar::Many(g, n) => {
                let items = match each(g, n, r, at) {
                    Ok(items) => items,
                    Err(e) => return e,
                };
                R::Got(Parts::List(items))
            }
            Grammar::Map(g, n) => {
                let mut map = Parts::Map(Vec::new());
                let items = match each(g, n, r, at) {
                    Ok(items) => items,
                    Err(e) => return e,
                };
                for p in items {
                    let key = match p.get("key") {
                        Some(Parts::Text(t) | Parts::Number(t)) => t.clone(),
                        _ if n.is_none() => continue, // passed over
                        _ => return R::No,
                    };
                    map.set(key, p.get("value").cloned().unwrap_or_default());
                }
                R::Got(map)
            }
            Grammar::Unless(g) => {
                let from = *at;
                let read = g.read(r, at);
                *at = from;
                match read {
                    R::Got(_) => R::No,
                    R::No => R::Got(Parts::None),
                    more => more,
                }
            }
            Grammar::Int(i) => {
                let bytes = match take(i.size) {
                    Ok(b) => b,
                    Err(e) => return e,
                };
                let held = i.held(bytes);
                if held < i.from || held > i.to {
                    return R::No;
                }
                *at += i.size;
                R::Got(Parts::Number(i.read(held).to_string()))
            }
            Grammar::Float(size, little) => {
                let mut bytes = match take(*size) {
                    Ok(b) => b.to_vec(),
                    Err(e) => return e,
                };
                if *little {
                    bytes.reverse();
                }
                *at += size;
                R::Got(Parts::from(match *size {
                    2 => half(u16::from_be_bytes([bytes[0], bytes[1]])),
                    4 => f32::from_be_bytes(bytes[..4].try_into().unwrap()) as f64,
                    _ => f64::from_be_bytes(bytes[..8].try_into().unwrap()),
                }))
            }
            Grammar::Text(n) | Grammar::Raw(n) => {
                let n = match n.read(r, at) {
                    Ok(n) => n,
                    Err(e) => return e,
                };
                let rest = &r.b[*at..];
                if rest.len() < n {
                    return if r.ended { R::No } else { R::More(at.saturating_add(n)) };
                }
                *at += n;
                R::Got(match self {
                    Grammar::Text(_) => text(&rest[..n]),
                    _ => Parts::Bytes(rest[..n].to_vec()),
                })
            }
            Grammar::Be(g, v) => {
                got!(g.read(r, at));
                R::Got(v.clone())
            }
            Grammar::Whole(g) => match got!(g.read(r, at)) {
                Parts::List(pieces) => {
                    let mut all = Vec::new();
                    let mut text = true;
                    for p in pieces {
                        match p {
                            Parts::Text(t) => all.extend(t.into_bytes()),
                            Parts::Bytes(b) => {
                                all.extend(b);
                                text = false;
                            }
                            _ => return R::No,
                        }
                    }
                    R::Got(if text { self::text(&all) } else { Parts::Bytes(all) })
                }
                _ => R::No,
            },
            Grammar::Ref(name) => {
                let Ok(f) = r.languages.get(name) else { return R::No };
                let Language::Grammar(g) = &*f else { return R::No };
                let here = (g as *const Grammar, *at);
                if r.deep >= DEEP || r.reading.contains(&here) {
                    return R::No;
                }
                r.deep += 1;
                r.reading.push(here);
                let read = g.read(r, at);
                r.reading.pop();
                r.deep -= 1;
                read
            }
        }
    }

    /// Write `v`, and whether it could be: `v` is the part this grammar is, if it `is` one, or else
    /// the parts around it, as a seq's pieces are given. One that could not leaves what it wrote
    /// for whoever tries next to take back.
    fn write(&self, v: &Parts, is: bool, w: &mut Writer) -> bool {
        match self {
            // itself: as a part, only for nothing or itself
            Grammar::Lit(s) => {
                let fits = match (is, v) {
                    (false, _) | (true, Parts::None) => true,
                    (true, Parts::Text(t)) => t.as_bytes() == &s[..],
                    (true, Parts::Bytes(b)) => b == s,
                    _ => false,
                };
                fits && put(w, s)
            }
            Grammar::Upto(s) => match said(v) {
                Some(t) if s.is_empty() || find(t.as_bytes(), s).is_none() => {
                    put(w, t.as_bytes()) && put(w, s)
                }
                _ => false,
            },
            Grammar::But(s) => match said(v) {
                Some(t) if !t.is_empty() && !t.bytes().any(|c| s.contains(&c)) => {
                    put(w, t.as_bytes())
                }
                _ => false,
            },
            Grammar::Seq(items, shape) => {
                // with no parts named, its one other piece is the part it is
                let mark = |g: &Grammar| matches!(g, Grammar::Lit(_) | Grammar::Unless(_));
                // a count is the length of what it counts
                let mut lengths = Vec::new();
                for item in items {
                    let Some(n) = counted_by(item) else { continue };
                    let of = match item {
                        Grammar::Part(name, _) => v.get(name),
                        _ => Some(v),
                    };
                    match of.and_then(length) {
                        Some(len) => lengths.push((n, len)),
                        None => return false,
                    }
                }
                items.iter().all(|g| match g {
                    Grammar::Part(name, count) if shape.counts(name) => {
                        let len = lengths.iter().find(|(n, _)| n == name).map(|(_, l)| *l);
                        len.is_some_and(|len| count.write(&Parts::from(len as u64), true, w))
                    }
                    g => g.write(v, shape.lone && is && !mark(g), w),
                })
            }
            Grammar::Part(name, g) => v.get(name).is_some_and(|v| g.write(v, true, w)),
            Grammar::Choice(gs) => gs.iter().any(|g| {
                let n = w.out.len();
                g.write(v, is, w) || {
                    w.out.truncate(n);
                    false
                }
            }),
            Grammar::Many(g, n) => match v.list() {
                Some(items) => {
                    n.as_ref().is_none_or(|n| n.write(items.len(), w))
                        && items.iter().all(|v| g.write(v, true, w))
                }
                None => false,
            },
            Grammar::Map(g, n) => {
                let Parts::Map(m) = v else { return false };
                n.as_ref().is_none_or(|n| n.write(m.len(), w))
                    && m.iter().all(|(k, v)| {
                        let pair =
                            Parts::map([("key", Parts::from(k.as_str())), ("value", v.clone())]);
                        g.write(&pair, false, w)
                    })
            }
            Grammar::Unless(_) => true,
            Grammar::Int(i) => match whole(v).map(|n| i.held_for(n)) {
                Some(held) if held >= i.from && held <= i.to => put(w, &i.bytes(held)),
                _ => false,
            },
            Grammar::Float(size, little) => {
                let Some(x) = v.number() else { return false };
                let mut bytes = match *size {
                    2 => match unhalf(x) {
                        Some(h) => h.to_be_bytes().to_vec(),
                        None => return false,
                    },
                    4 if (x as f32) as f64 == x => (x as f32).to_be_bytes().to_vec(),
                    4 => return false,
                    _ => x.to_be_bytes().to_vec(),
                };
                if *little {
                    bytes.reverse();
                }
                put(w, &bytes)
            }
            Grammar::Text(n) => match v {
                Parts::Text(t) => n.write(t.len(), w) && put(w, t.as_bytes()),
                _ => false,
            },
            // text too, as its bytes: Lua cannot say which a string is
            Grammar::Raw(n) => match v {
                Parts::Bytes(b) => n.write(b.len(), w) && put(w, b),
                Parts::Text(t) => n.write(t.len(), w) && put(w, t.as_bytes()),
                _ => false,
            },
            Grammar::Be(g, be) => (!is || v == be) && g.write(v, false, w),
            Grammar::Whole(g) => match v {
                Parts::Text(_) | Parts::Bytes(_) => g.write(&Parts::List(vec![v.clone()]), true, w),
                _ => false,
            },
            Grammar::Ref(name) => {
                let Ok(f) = w.languages.get(name) else { return false };
                let Language::Grammar(g) = &*f else { return false };
                let here = (g as *const Grammar, v as *const Parts);
                if w.deep >= DEEP || w.writing.contains(&here) {
                    return false;
                }
                w.deep += 1;
                w.writing.push(here);
                let wrote = g.write(v, is, w);
                w.writing.pop();
                w.deep -= 1;
                wrote
            }
        }
    }
}

/// the pieces a grammar is made of
const PIECES: [&str; 16] = [
    "lit", "upto", "but", "seq", "part", "choice", "many", "map", "unless", "int", "float", "text",
    "raw", "be", "ref", "whole",
];

impl Count {
    fn of(p: &Parts) -> Result<Count, String> {
        match p {
            Parts::Number(_) => match whole(p) {
                Some(n) if n >= 0 => Ok(Count::Fixed(n as usize)),
                _ => Err("a count is a whole number, 0 or more".into()),
            },
            Parts::Text(name) => Ok(Count::Named(name.clone())),
            _ => Ok(Count::Read(Box::new(Grammar::of(p)?))),
        }
    }

    /// how many, as read
    fn read(&self, r: &mut Reader, at: &mut usize) -> Result<usize, R> {
        match self {
            Count::Fixed(n) => Ok(*n),
            Count::Named(name) => {
                r.known.iter().rev().find(|(n, _)| n == name).map(|(_, n)| *n).ok_or(R::No)
            }
            Count::Read(g) => match g.read(r, at) {
                R::Got(p) => whole(&p).and_then(|n| usize::try_from(n).ok()).ok_or(R::No),
                other => Err(other),
            },
        }
    }

    /// how many, written
    fn write(&self, n: usize, w: &mut Writer) -> bool {
        match self {
            Count::Fixed(m) => *m == n,
            Count::Named(_) => true, // the seq has written it
            Count::Read(g) => g.write(&Parts::from(n as u64), true, w),
        }
    }
}

impl Int {
    fn of(p: &Parts) -> Result<Int, String> {
        let size = number(p, "size").unwrap_or(1.0) as usize;
        if !(1..=8).contains(&size) {
            return Err("an int is 1 to 8 bytes".into());
        }
        let signed = matches!(p.get("signed"), Some(Parts::Bool(true)));
        let bits = 8 * size as u32;
        let (low, high) = match signed {
            true => (-(1i128 << (bits - 1)), (1i128 << (bits - 1)) - 1),
            false => (0, (1i128 << bits) - 1),
        };
        let bound = |name| p.get(name).and_then(whole);
        Ok(Int {
            size,
            signed,
            little: little(p)?,
            from: bound("from").unwrap_or(low).max(low),
            to: bound("to").unwrap_or(high).min(high),
            base: bound("base").unwrap_or(0),
            negative: matches!(p.get("negative"), Some(Parts::Bool(true))),
        })
    }

    /// the number read, of what the bytes hold
    fn read(&self, held: i128) -> i128 {
        match self.negative {
            true => self.base - held,
            false => held - self.base,
        }
    }

    /// what the bytes hold, of the number
    fn held_for(&self, n: i128) -> i128 {
        match self.negative {
            true => self.base - n,
            false => n + self.base,
        }
    }

    /// the number the bytes hold
    fn held(&self, bytes: &[u8]) -> i128 {
        let mut b = [0u8; 16];
        let pad = if self.signed
            && (if self.little { bytes[self.size - 1] } else { bytes[0] }) & 0x80 != 0
        {
            0xff
        } else {
            0
        };
        b.fill(pad);
        for (k, byte) in bytes.iter().enumerate() {
            let at = if self.little { k } else { self.size - 1 - k };
            b[at] = *byte;
        }
        i128::from_le_bytes(b)
    }

    /// the bytes that hold the number
    fn bytes(&self, held: i128) -> Vec<u8> {
        let mut b = held.to_le_bytes()[..self.size].to_vec();
        if !self.little {
            b.reverse();
        }
        b
    }
}

/// `g` read again and again: as many times as `n` says, or while it reads and moves on
fn each(g: &Grammar, n: &Option<Count>, r: &mut Reader, at: &mut usize) -> Result<Vec<Parts>, R> {
    let mut items = Vec::new();
    match n {
        Some(n) => {
            for _ in 0..n.read(r, at)? {
                match g.read(r, at) {
                    R::Got(p) => items.push(p),
                    other => return Err(other),
                }
            }
        }
        None => loop {
            let from = *at;
            match g.read(r, at) {
                R::Got(p) if *at > from => items.push(p),
                more @ R::More(_) => return Err(more),
                _ => {
                    *at = from;
                    break;
                }
            }
        },
    }
    Ok(items)
}

/// the part a piece's count names, if it names one
fn counted_by(g: &Grammar) -> Option<&str> {
    match g {
        Grammar::Part(_, g) => g.count_name(),
        g => g.count_name(),
    }
}

/// how long a part is, as a count counts it: its bytes, or its parts
fn length(p: &Parts) -> Option<usize> {
    match p {
        Parts::Text(t) => Some(t.len()),
        Parts::Bytes(b) => Some(b.len()),
        Parts::List(l) => Some(l.len()),
        Parts::Map(m) => Some(m.len()),
        _ => None,
    }
}

impl Grammar {
    /// the part this piece's count names, if it names one
    fn count_name(&self) -> Option<&str> {
        match self {
            Grammar::Text(Count::Named(n))
            | Grammar::Raw(Count::Named(n))
            | Grammar::Many(_, Some(Count::Named(n)))
            | Grammar::Map(_, Some(Count::Named(n))) => Some(n),
            _ => None,
        }
    }
}

fn put(w: &mut Writer, b: &[u8]) -> bool {
    w.out.extend_from_slice(b);
    true
}

fn find(hay: &[u8], s: &[u8]) -> Option<usize> {
    (!s.is_empty()).then(|| hay.windows(s.len()).position(|w| w == s)).flatten()
}

fn text(b: &[u8]) -> Parts {
    Parts::Text(String::from_utf8_lossy(b).into_owned())
}

/// bytes as they are: text if they are text
fn leaf(b: &[u8]) -> Parts {
    match std::str::from_utf8(b) {
        Ok(t) => Parts::Text(t.into()),
        Err(_) => Parts::Bytes(b.to_vec()),
    }
}

/// a leaf as the text it is written as
fn said(v: &Parts) -> Option<String> {
    match v {
        Parts::Text(t) | Parts::Number(t) => Some(t.clone()),
        Parts::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

/// a whole number, as a part holds it
fn whole(p: &Parts) -> Option<i128> {
    let Parts::Number(n) = p else { return None };
    n.parse::<i128>().ok().or_else(|| {
        let x: f64 = n.parse().ok()?;
        (x.fract() == 0.0 && x.abs() < 1e38).then_some(x as i128)
    })
}

fn number(p: &Parts, name: &str) -> Option<f64> {
    p.get(name).and_then(Parts::number)
}

fn little(p: &Parts) -> Result<bool, String> {
    match p.get("order").and_then(Parts::text) {
        None | Some("big") => Ok(false),
        Some("little") => Ok(true),
        Some(o) => Err(format!("an order is big or little, not {o}")),
    }
}

/// a number of 2 bytes, as IEEE 754 holds it
fn half(h: u16) -> f64 {
    let (sign, exp, frac) = ((h >> 15) as i32, ((h >> 10) & 0x1f) as i32, (h & 0x3ff) as f64);
    let x = match exp {
        0 => frac * 2f64.powi(-24),
        31 if frac == 0.0 => f64::INFINITY,
        31 => f64::NAN,
        _ => (1.0 + frac / 1024.0) * 2f64.powi(exp - 15),
    };
    if sign == 1 {
        -x
    } else {
        x
    }
}

/// a number in 2 bytes, if they hold it exactly
fn unhalf(x: f64) -> Option<u16> {
    let sign = if x.is_sign_negative() { 0x8000 } else { 0 };
    let a = x.abs();
    let h = if a == 0.0 {
        0
    } else if a.is_infinite() {
        0x7c00
    } else if a.is_nan() {
        return None;
    } else {
        let e = a.log2().floor() as i32;
        let (exp, frac) = match e {
            -14..=15 => (e + 15, (a / 2f64.powi(e) - 1.0) * 1024.0),
            -24..=-15 => (0, a / 2f64.powi(-24)),
            _ => return None,
        };
        if frac.fract() != 0.0 || frac >= 1024.0 {
            return None;
        }
        ((exp as u16) << 10) | frac as u16
    };
    Some(sign | h).filter(|&h| half(h) == x)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    fn grammar(v: Value) -> Grammar {
        Grammar::of(&Parts::from(v)).unwrap()
    }

    fn parts(v: Value) -> Parts {
        Parts::deserialize(v).unwrap()
    }

    /// a grammar, read and written whole, as a language with no others beside it
    trait Whole {
        fn parts(&self, b: &[u8]) -> Option<Parts>;
        fn bytes(&self, p: &Parts) -> Option<Vec<u8>>;
    }

    impl Whole for Grammar {
        fn parts(&self, b: &[u8]) -> Option<Parts> {
            Language::Grammar(self.clone()).parts(b, &Languages::default())
        }
        fn bytes(&self, p: &Parts) -> Option<Vec<u8>> {
            Language::Grammar(self.clone()).bytes(p, &Languages::default())
        }
    }

    /// kv as town says it in resident.lua
    fn kv() -> Grammar {
        grammar(json!({ "map": { "choice": [
            { "seq": [
                { "part": ["key", { "but": "=\n" }] },
                { "lit": "=" },
                { "part": ["value", { "upto": "\n" }] },
            ] },
            { "upto": "\n" },
        ] } }))
    }

    // kv reads lines of key=value, passing over the rest, and a last line without its end
    #[test]
    fn kv_reads_its_pairs() {
        let read = kv().parts(b"# shared\nbg=#000\n\nfg=#fff\nurl=a=b\nlast=1").unwrap();
        assert_eq!(read, parts(json!({ "bg": "#000", "fg": "#fff", "url": "a=b", "last": "1" })));
        assert_eq!(kv().parts(b""), Some(parts(json!({}))));
    }

    // what is read is written as it was, but what was passed over; and what cannot be written in
    // a grammar is not written
    #[test]
    fn kv_writes_what_it_reads() {
        let read = parts(json!({ "bg": "#000", "tint": 0.9 }));
        let bytes = kv().bytes(&read).unwrap();
        assert_eq!(bytes, b"bg=#000\ntint=0.9\n");
        assert_eq!(kv().parts(&bytes), Some(parts(json!({ "bg": "#000", "tint": "0.9" }))));
        assert_eq!(kv().bytes(&parts(json!({ "a=b": "c" }))), None, "a key holds no =");
        assert_eq!(kv().bytes(&parts(json!({ "a": "two\nlines" }))), None);
        assert_eq!(kv().bytes(&parts(json!("text"))), None, "kv is parts");
    }

    // bytes that are not all of a grammar are not of it; a choice that fails part way puts back
    // what it read, and so does one that fails part way as it writes
    #[test]
    fn a_grammar_reads_all_or_nothing() {
        let g = grammar(json!({ "seq": [{ "lit": "%" }, { "part": ["n", { "but": " " }] }] }));
        assert_eq!(g.parts(b"%12"), Some(parts(json!({ "n": "12" }))));
        assert_eq!(g.parts(b"%12 more"), None);
        let g = grammar(json!({ "many": { "choice": [
            { "seq": [{ "lit": "a" }, { "part": ["b", { "lit": "b" }] }] },
            { "part": ["a", { "upto": "," }] },
        ] } }));
        assert_eq!(g.parts(b"ax,y"), Some(parts(json!([{ "a": "ax" }, { "a": "y" }]))));
        assert_eq!(g.bytes(&parts(json!([{ "a": "x" }]))), Some(b"x,".to_vec()));
    }
}
