//! A body: what a word holds. Parts, or the JSON they were said in, read only when a part is
//! asked for; written as JSON, it goes on as it came. Another language writes its parts anew.

use std::borrow::Cow;
use std::cell::OnceCell;
use std::fmt;
use std::rc::Rc;

use crate::parts::Parts;

/// Shared by every copy, so a body heard by many is read once.
#[derive(Clone)]
pub struct Body(Rc<Held>);

struct Held {
    /// the JSON it was said in
    said: Option<Box<[u8]>>,
    parts: OnceCell<Parts>,
}

impl Body {
    /// a body of nothing
    pub fn none() -> Body {
        Body::of(Parts::None)
    }

    /// a body of these parts
    pub fn of(parts: impl Into<Parts>) -> Body {
        Body(Rc::new(Held { said: None, parts: OnceCell::from(parts.into()) }))
    }

    /// a body of JSON, as it was said
    pub fn said(bytes: impl Into<Box<[u8]>>) -> Body {
        Body(Rc::new(Held { said: Some(bytes.into()), parts: OnceCell::new() }))
    }

    /// its parts: JSON that is not JSON is nothing
    pub fn parts(&self) -> &Parts {
        self.0.parts.get_or_init(|| {
            let bytes = self.0.said.as_ref().expect("a body is parts, or said");
            serde_json::from_slice(bytes).unwrap_or_default()
        })
    }

    /// the part named `name`
    pub fn part(&self, name: &str) -> Option<&Parts> {
        self.parts().get(name)
    }

    /// Whether it is nothing: as said, if it was said, without reading it. A body that cannot be
    /// read is not nothing, so a fact of it stays, kept as it was said.
    pub fn is_none(&self) -> bool {
        match &self.0.said {
            Some(bytes) => bytes.trim_ascii() == b"null",
            _ => self.parts().is_none(),
        }
    }

    /// the whole body, to look into in a test
    #[cfg(test)]
    pub fn value(&self) -> serde_json::Value {
        serde_json::to_value(self.parts()).unwrap_or_default()
    }

    /// as JSON: as it was said, if it was
    pub fn json(&self) -> Cow<'_, [u8]> {
        match &self.0.said {
            Some(bytes) => Cow::Borrowed(bytes),
            None => Cow::Owned(serde_json::to_vec(self.parts()).unwrap_or_else(|_| b"null".into())),
        }
    }
}

impl Default for Body {
    fn default() -> Body {
        Body::none()
    }
}

impl fmt::Debug for Body {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        self.parts().fmt(f)
    }
}

impl PartialEq for Body {
    fn eq(&self, other: &Body) -> bool {
        self.parts() == other.parts()
    }
}

/// a body of text
impl PartialEq<&str> for Body {
    fn eq(&self, other: &&str) -> bool {
        self.parts().text() == Some(*other)
    }
}

impl PartialEq<Parts> for Body {
    fn eq(&self, other: &Parts) -> bool {
        self.parts() == other
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // a body said in JSON is read only when asked, and written as it came; one of parts, anew
    #[test]
    fn a_body_goes_on_as_it_came() {
        let b = Body::said(&b"{ \"b\": 1,  \"a\": [true] }"[..]);
        assert!(b.0.parts.get().is_none(), "not read until asked");
        assert_eq!(&*b.json(), &b"{ \"b\": 1,  \"a\": [true] }"[..]);
        assert_eq!(b.part("b").and_then(Parts::number), Some(1.0));
        assert_eq!(&*Body::of(b.parts().clone()).json(), br#"{"b":1,"a":[true]}"#);
        assert!(Body::none().is_none() && !Body::of("").is_none());
        let unread = Body::said(&br#"{"n":1e400}"#[..]);
        assert!(!unread.is_none(), "what cannot be read is not nothing");
        assert!(unread.parts().is_none() && !unread.is_none(), "it reads as nothing, read or not");
        assert_eq!(&*unread.json(), &br#"{"n":1e400}"#[..], "kept as said");
        assert!(Body::said(&b" null"[..]).is_none());
    }
}
