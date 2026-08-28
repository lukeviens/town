//! A word: a kind, a tense, a body. Only present words fold into the present;
//! past/future pass through.

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Word {
    pub kind: String,
    #[serde(default = "present")]
    pub tense: Tense,
    #[serde(default)]
    pub body: Value,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Tense {
    Past,
    Present,
    Future,
}

fn present() -> Tense {
    Tense::Present
}

impl Word {
    pub fn from_line(line: &str) -> Option<Word> {
        serde_json::from_str(line).ok()
    }

    pub fn to_line(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    /// A fact: a present-tense word with a kind. Only facts fold into the present and are
    /// worth keeping in the log; events (past) and intentions (future) just pass through.
    pub fn is_fact(&self) -> bool {
        self.tense == Tense::Present && !self.kind.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tense_defaults_to_present() {
        let w = Word::from_line(r#"{"kind":"place","body":{}}"#).unwrap();
        assert_eq!(w.tense, Tense::Present);
    }

    #[test]
    fn a_word_survives_a_roundtrip() {
        let w = Word::from_line(r#"{"kind":"pick","tense":"future","body":{"what":"apps"}}"#).unwrap();
        assert_eq!(w.tense, Tense::Future);
        let again = Word::from_line(&w.to_line()).unwrap();
        assert_eq!(again.kind, "pick");
        assert_eq!(again.tense, Tense::Future);
        assert_eq!(again.body["what"], "apps");
    }
}
