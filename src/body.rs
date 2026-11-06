//! A body: what a word contains.
//!
//! - **parts**: read only when one is asked for; till then, the JSON it was said in.
//! - **written**: as JSON, it goes on as it came; another language writes its parts anew.

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

    /// its parts; bytes that are not JSON read as nothing
    pub fn parts(&self) -> &Parts {
        self.0.parts.get_or_init(|| {
            let bytes = self.0.said.as_ref().expect("a body is parts, or said");
            crate::parts::json(bytes).unwrap_or_default()
        })
    }

    /// a part, by name
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

    /// Whether it was said and cannot be read: nothing as parts, though not said as nothing.
    fn unread(&self) -> bool {
        self.parts().is_none() && !self.is_none()
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

impl fmt::Debug for Body {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        self.parts().fmt(f)
    }
}

impl PartialEq for Body {
    fn eq(&self, other: &Body) -> bool {
        // the same bytes are the same parts, unread
        if let (Some(a), Some(b)) = (&self.0.said, &other.0.said) {
            if a == b {
                return true;
            }
        }
        // one that cannot be read is the same only as the same bytes, above
        !self.unread() && !other.unread() && self.parts() == other.parts()
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

    // two that cannot be read are the same only as the same bytes: neither is nothing
    #[test]
    fn what_cannot_be_read_is_itself() {
        let (a, b) = (Body::said(&b"{\"n\":1e400}"[..]), Body::said(&b"[2e999]"[..]));
        assert!(a != b && a == Body::said(&b"{\"n\":1e400}"[..]));
        assert!(a != Body::none() && Body::said(&b"null"[..]) == Body::none());
    }

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
        assert!(unread.parts().is_none(), "though it reads as nothing");
        assert_eq!(&*unread.json(), &br#"{"n":1e400}"#[..], "kept as said");
        assert!(Body::said(&b" null"[..]).is_none());
    }
}
