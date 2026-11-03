//! A word: a kind, a tense, a body. Only present words fold into the present;
//! past/future pass through.

use crate::body::Body;

#[derive(Clone, Debug)]
pub struct Word {
    pub kind: String,
    pub tense: Tense,
    pub body: Body,
    /// When it was said, ms since the epoch; the town stamps it as it talks. 0 = not yet said.
    pub when: u64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Tense {
    Past,
    Present,
    Future,
}

impl Tense {
    pub fn name(self) -> &'static str {
        match self {
            Tense::Past => "past",
            Tense::Present => "present",
            Tense::Future => "future",
        }
    }

    pub fn named(name: &str) -> Option<Tense> {
        [Tense::Past, Tense::Present, Tense::Future].into_iter().find(|t| t.name() == name)
    }
}

impl Word {
    /// A word not yet said: town stamps `when` as it talks it.
    pub fn new(kind: impl Into<String>, tense: Tense, body: Body) -> Word {
        Word { kind: kind.into(), tense, body, when: 0 }
    }

    /// A fact: a present-tense word with a kind. Only facts fold into the present and are
    /// worth keeping in the log; events (past) and intentions (future) just pass through.
    pub fn is_fact(&self) -> bool {
        self.tense == Tense::Present && !self.kind.is_empty()
    }
}
