//! Parts: what a body contains, in town's own terms: nothing, text, bytes, a number, true or false,
//! a list, or parts by name. A language reads bytes as parts and writes parts as bytes, each its
//! own way.

use std::fmt;

use serde::de::{MapAccess, SeqAccess, Visitor};
use serde::ser::{SerializeMap, SerializeSeq};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

#[derive(Default)]
pub enum Parts {
    #[default]
    None,
    Text(String),
    /// bytes that are not text
    Bytes(Vec<u8>),
    /// a number, as the text it is written as, so none is changed in passing
    Number(String),
    Bool(bool),
    List(Vec<Parts>),
    /// parts by name, in the order they came
    Map(Vec<(String, Parts)>),
}

/// A step into what parts contain, however deep: the stack grows as it must, on the heap, so parts
/// as deep as memory allows are walked as any are, and none is too deep.
pub fn deep<R>(step: impl FnOnce() -> R) -> R {
    stacker::maybe_grow(64 * 1024, 1024 * 1024, step)
}

/// What JSON says, however deep: read with no depth of serde_json's own, as each step into parts
/// grows the stack as it must (`deep`), and serde_json passes over what it keeps whole, a word's
/// body, without a step.
pub fn json<'a, T: Deserialize<'a>>(bytes: &'a [u8]) -> serde_json::Result<T> {
    let mut read = serde_json::Deserializer::from_slice(bytes);
    read.disable_recursion_limit();
    let it = T::deserialize(&mut read)?;
    read.end()?;
    Ok(it)
}

impl Clone for Parts {
    fn clone(&self) -> Parts {
        deep(|| match self {
            Parts::None => Parts::None,
            Parts::Text(t) => Parts::Text(t.clone()),
            Parts::Bytes(b) => Parts::Bytes(b.clone()),
            Parts::Number(n) => Parts::Number(n.clone()),
            Parts::Bool(b) => Parts::Bool(*b),
            Parts::List(l) => Parts::List(l.clone()),
            Parts::Map(m) => Parts::Map(m.clone()),
        })
    }
}

impl fmt::Debug for Parts {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        deep(|| match self {
            Parts::None => f.write_str("None"),
            Parts::Text(t) => f.debug_tuple("Text").field(t).finish(),
            Parts::Bytes(b) => f.debug_tuple("Bytes").field(b).finish(),
            Parts::Number(n) => f.debug_tuple("Number").field(n).finish(),
            Parts::Bool(b) => f.debug_tuple("Bool").field(b).finish(),
            Parts::List(l) => f.debug_tuple("List").field(l).finish(),
            Parts::Map(m) => f.debug_tuple("Map").field(m).finish(),
        })
    }
}

/// Let go without a step for each that is in another: what each contains is taken out first,
/// so parts however deep go as flat ones do.
impl Drop for Parts {
    fn drop(&mut self) {
        let held = |p: &mut Parts, into: &mut Vec<Parts>| match p {
            Parts::List(l) => into.append(l),
            Parts::Map(m) => into.extend(m.drain(..).map(|(_, p)| p)),
            _ => {}
        };
        let mut going = Vec::new();
        held(self, &mut going);
        while let Some(mut p) = going.pop() {
            held(&mut p, &mut going);
        }
    }
}

impl Parts {
    /// parts by name, from pairs
    pub fn map<N: Into<String>, P: Into<Parts>>(pairs: impl IntoIterator<Item = (N, P)>) -> Parts {
        let mut map = Parts::Map(Vec::new());
        for (name, part) in pairs {
            map.set(name, part);
        }
        map
    }

    pub fn is_none(&self) -> bool {
        matches!(self, Parts::None)
    }

    /// a part, by name, if these are parts by name
    pub fn get(&self, name: &str) -> Option<&Parts> {
        match self {
            Parts::Map(m) => m.iter().find(|(n, _)| n == name).map(|(_, p)| p),
            _ => None,
        }
    }

    /// name a part: in the place of one of the same name, or after the rest. Nothing if these are
    /// not parts by name.
    pub fn set(&mut self, name: impl Into<String>, part: impl Into<Parts>) {
        let (name, part) = (name.into(), part.into());
        if let Parts::Map(m) = self {
            match m.iter_mut().find(|(n, _)| *n == name) {
                Some((_, p)) => *p = part,
                None => m.push((name, part)),
            }
        }
    }

    /// take away a part, by name
    pub fn remove(&mut self, name: &str) {
        if let Parts::Map(m) = self {
            m.retain(|(n, _)| n != name);
        }
    }

    pub fn text(&self) -> Option<&str> {
        match self {
            Parts::Text(t) => Some(t),
            _ => None,
        }
    }

    pub fn number(&self) -> Option<f64> {
        match self {
            Parts::Number(n) => n.parse().ok(),
            _ => None,
        }
    }

    /// the list these are, taken, for one that owns them
    pub fn into_list(mut self) -> Option<Vec<Parts>> {
        match &mut self {
            Parts::List(l) => Some(std::mem::take(l)),
            _ => None,
        }
    }

    /// the parts by name these are, taken, for one that owns them
    pub fn into_map(mut self) -> Option<Vec<(String, Parts)>> {
        match &mut self {
            Parts::Map(m) => Some(std::mem::take(m)),
            _ => None,
        }
    }

    pub fn list(&self) -> Option<&[Parts]> {
        match self {
            Parts::List(l) => Some(l),
            _ => None,
        }
    }
}

/// The same parts: by name in any order, and a number by what it is, not how it is written.
impl PartialEq for Parts {
    fn eq(&self, other: &Parts) -> bool {
        deep(|| match (self, other) {
            (Parts::None, Parts::None) => true,
            (Parts::Text(a), Parts::Text(b)) => a == b,
            (Parts::Bytes(a), Parts::Bytes(b)) => a == b,
            (Parts::Number(a), Parts::Number(b)) => same_number(a, b),
            (Parts::Bool(a), Parts::Bool(b)) => a == b,
            (Parts::List(a), Parts::List(b)) => a == b,
            (Parts::Map(a), Parts::Map(b)) => {
                a.len() == b.len() && a.iter().all(|(n, p)| other.get(n) == Some(p))
            }
            _ => false,
        })
    }
}

/// Whether two numbers, as written, are the same number: two whole ones exactly, however big, so
/// none is the same as its neighbour; any other by its value, so `1` is `1.0`.
fn same_number(a: &str, b: &str) -> bool {
    match (whole(a), whole(b)) {
        (Some(a), Some(b)) => a == b,
        _ => a == b || a.parse::<f64>().ok().is_some_and(|x| b.parse::<f64>().ok() == Some(x)),
    }
}

/// A whole number as written, by its sign and its digits, none leading: so `-0` is `0`, and `007`
/// is `7`, however many digits. None if it is not whole.
fn whole(n: &str) -> Option<(bool, &str)> {
    let (minus, digits) = n.strip_prefix('-').map_or((false, n), |d| (true, d));
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let digits = digits.trim_start_matches('0');
    Some((minus && !digits.is_empty(), digits))
}

impl From<&str> for Parts {
    fn from(t: &str) -> Parts {
        Parts::Text(t.into())
    }
}

impl From<String> for Parts {
    fn from(t: String) -> Parts {
        Parts::Text(t)
    }
}

impl From<bool> for Parts {
    fn from(b: bool) -> Parts {
        Parts::Bool(b)
    }
}

impl From<i64> for Parts {
    fn from(n: i64) -> Parts {
        Parts::Number(n.to_string())
    }
}

impl From<u64> for Parts {
    fn from(n: u64) -> Parts {
        Parts::Number(n.to_string())
    }
}

/// a number that is none (NaN, or without end) is nothing
impl From<f64> for Parts {
    fn from(n: f64) -> Parts {
        match n.is_finite() {
            true => Parts::Number(n.to_string()),
            false => Parts::None,
        }
    }
}

/// parts as any serde language writes them: what the JSON language is built on
impl Serialize for Parts {
    fn serialize<S: Serializer>(&self, to: S) -> Result<S::Ok, S::Error> {
        deep(|| match self {
            Parts::None => to.serialize_unit(),
            Parts::Text(t) => to.serialize_str(t),
            Parts::Bytes(b) => to.serialize_bytes(b),
            Parts::Number(n) => match (n.parse::<i64>(), n.parse::<u64>(), n.parse::<f64>()) {
                (Ok(i), _, _) => to.serialize_i64(i),
                (_, Ok(u), _) => to.serialize_u64(u),
                (_, _, Ok(f)) => to.serialize_f64(f),
                _ => to.serialize_str(n),
            },
            Parts::Bool(b) => to.serialize_bool(*b),
            Parts::List(l) => {
                let mut seq = to.serialize_seq(Some(l.len()))?;
                l.iter().try_for_each(|p| seq.serialize_element(p))?;
                seq.end()
            }
            Parts::Map(m) => {
                let mut map = to.serialize_map(Some(m.len()))?;
                m.iter().try_for_each(|(n, p)| map.serialize_entry(n, p))?;
                map.end()
            }
        })
    }
}

/// parts as any serde language reads them
impl<'de> Deserialize<'de> for Parts {
    fn deserialize<D: Deserializer<'de>>(from: D) -> Result<Parts, D::Error> {
        from.deserialize_any(Reading)
    }
}

struct Reading;

impl<'de> Visitor<'de> for Reading {
    type Value = Parts;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("parts")
    }
    fn visit_unit<E>(self) -> Result<Parts, E> {
        Ok(Parts::None)
    }
    fn visit_none<E>(self) -> Result<Parts, E> {
        Ok(Parts::None)
    }
    fn visit_some<D: Deserializer<'de>>(self, d: D) -> Result<Parts, D::Error> {
        Parts::deserialize(d)
    }
    fn visit_bool<E>(self, b: bool) -> Result<Parts, E> {
        Ok(Parts::Bool(b))
    }
    fn visit_i64<E>(self, n: i64) -> Result<Parts, E> {
        Ok(n.into())
    }
    fn visit_u64<E>(self, n: u64) -> Result<Parts, E> {
        Ok(n.into())
    }
    fn visit_f64<E>(self, n: f64) -> Result<Parts, E> {
        Ok(n.into())
    }
    fn visit_str<E>(self, t: &str) -> Result<Parts, E> {
        Ok(t.into())
    }
    fn visit_string<E>(self, t: String) -> Result<Parts, E> {
        Ok(t.into())
    }
    fn visit_bytes<E>(self, b: &[u8]) -> Result<Parts, E> {
        Ok(Parts::Bytes(b.to_vec()))
    }
    fn visit_byte_buf<E>(self, b: Vec<u8>) -> Result<Parts, E> {
        Ok(Parts::Bytes(b))
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Parts, A::Error> {
        deep(|| {
            let mut l = Vec::new();
            while let Some(p) = seq.next_element()? {
                l.push(p);
            }
            Ok(Parts::List(l))
        })
    }
    /// as `set` names each, the last of a name in the place of the first, but each found once, by
    /// where it is: however many parts, each costs the same
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Parts, A::Error> {
        deep(|| {
            let (mut m, mut at) = (Vec::new(), std::collections::HashMap::new());
            while let Some((n, p)) = map.next_entry::<String, Parts>()? {
                match at.get(&n) {
                    Some(&i) => m[i] = (n, p),
                    None => {
                        at.insert(n.clone(), m.len());
                        m.push((n, p));
                    }
                }
            }
            Ok(Parts::Map(m))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // two whole numbers are the same only if they are, however big; a whole one and a fraction of
    // one value are the same
    #[test]
    fn a_number_is_the_same_only_as_itself() {
        let n = |t: &str| Parts::Number(t.into());
        assert_ne!(n("9007199254740992"), n("9007199254740993"), "past where a float is exact");
        assert_ne!(n("1793609215000000001"), n("1793609215000000002"), "as a file's ns are");
        assert_eq!(n("1"), n("1.0"));
        assert_eq!(n("12"), n("12"));
        let big = "1".repeat(60);
        assert_ne!(n(&format!("{big}1")), n(&format!("{big}2")), "past where i128 is");
        assert_eq!(n(&format!("{big}1")), n(&format!("00{big}1")));
        assert_eq!(n("-0"), n("0"));
        assert_ne!(n("-7"), n("7"));
    }

    // JSON is read however deep, as a word and as its body, with no depth of serde_json's own
    #[test]
    fn json_is_read_however_deep() {
        let deep = 20_000;
        let body = format!("{}1{}", "[".repeat(deep), "]".repeat(deep));
        let line = format!(r#"{{"kind":"deep","tense":"past","body":{body}}}"#);
        let w = crate::voice::read(&line).expect("a word, however deep");
        let mut parts = w.body.parts();
        for _ in 0..deep {
            parts = &parts.list().expect("a list in a list")[0];
        }
        assert_eq!(parts, &Parts::from(1i64), "all the way down");
        assert!(json::<Parts>(body.as_bytes()).is_ok(), "and read as parts alone");
    }

    // parts by name are the same in any order, a later one of a name takes the earlier's place,
    // and a number is what it is however it is written
    #[test]
    fn parts_are_what_they_hold() {
        let a = Parts::map([("bg", Parts::from("#000")), ("n", 1i64.into())]);
        let b = Parts::map([("n", Parts::Number("1.0".into())), ("bg", "#000".into())]);
        assert_eq!(a, b);
        let mut c = a.clone();
        c.set("bg", "#111");
        assert_eq!(c.get("bg").and_then(Parts::text), Some("#111"));
        assert_ne!(a, c);
        assert!(Parts::from(f64::NAN).is_none());
    }

    // parts by name are read as `set` names them, the last of a name in the place of the first;
    // and each found once, so many names read as fast as few, each
    #[test]
    fn parts_by_name_are_read_once_each() {
        let read: Parts = serde_json::from_str(r#"{"a":1,"b":2,"a":3}"#).unwrap();
        let Parts::Map(m) = &read else { panic!("parts by name") };
        let names: Vec<&str> = m.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!((names, read.get("a")), (vec!["a", "b"], Some(&Parts::from(3i64))));
        // a name looked for among all held costs n² compares; found by where it is, n
        let many = |n: usize| {
            let body: Vec<String> = (0..n).map(|i| format!("\"k{i}\":0")).collect();
            format!("{{{}}}", body.join(","))
        };
        // the least of three, so a busy moment is not taken for the work
        let took = |n: usize| {
            let json = many(n);
            let once = || {
                let t = std::time::Instant::now();
                let read: Parts = serde_json::from_str(&json).unwrap();
                assert_eq!(read.get(&format!("k{}", n - 1)), Some(&Parts::from(0i64)));
                t.elapsed()
            };
            (0..3).map(|_| once()).min().unwrap()
        };
        let (few, lots) = (took(2_000), took(20_000));
        assert!(lots < few * 40, "ten times the names, ten times the work: {few:?}, {lots:?}");
    }
}
