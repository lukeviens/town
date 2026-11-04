//! Parts: what a body holds, in town's own terms: nothing, text, bytes, a number, true or false, a
//! list, or parts by name. A language reads bytes as parts and writes parts as bytes, each its own
//! way.

use std::fmt;

use serde::de::{MapAccess, SeqAccess, Visitor};
use serde::ser::{SerializeMap, SerializeSeq};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

#[derive(Clone, Debug, Default)]
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
        match (self, other) {
            (Parts::None, Parts::None) => true,
            (Parts::Text(a), Parts::Text(b)) => a == b,
            (Parts::Bytes(a), Parts::Bytes(b)) => a == b,
            (Parts::Number(a), Parts::Number(b)) => a == b || self.number() == other.number(),
            (Parts::Bool(a), Parts::Bool(b)) => a == b,
            (Parts::List(a), Parts::List(b)) => a == b,
            (Parts::Map(a), Parts::Map(b)) => {
                a.len() == b.len() && a.iter().all(|(n, p)| other.get(n) == Some(p))
            }
            _ => false,
        }
    }
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
        match self {
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
        }
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
        let mut l = Vec::new();
        while let Some(p) = seq.next_element()? {
            l.push(p);
        }
        Ok(Parts::List(l))
    }
    /// as `set` names each, the last of a name in the place of the first, but each found once, by
    /// where it is: however many parts, each costs the same
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Parts, A::Error> {
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
