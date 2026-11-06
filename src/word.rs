//! A word: a kind, a tense, a body, and when it was said. PROTOCOL.md's *Words*.
//!
//! - **present**: folds into the present.
//! - **past**, **future**: pass through.

use crate::body::Body;

#[derive(Clone, Debug)]
pub struct Word {
    pub kind: String,
    pub tense: Tense,
    pub body: Body,
    /// When it was said, in ms since the epoch, stamped by town as it comes in by the door, or is
    /// talked; 0 until then.
    pub when: u64,
    /// Who said it, or whose error it says, by the number town gave them; 0 is town itself.
    /// Town's own to know: no language says it.
    pub by: u64,
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
        Word { kind: kind.into(), tense, body, when: 0, by: 0 }
    }

    /// That the present is told, and what follows is now: a bodiless past word at `""`, where the
    /// listener stands. Told to a resident; said by one, that it has said again each fact it has
    /// (`Town::said`). Never talked, and `""` is no kind.
    pub fn now() -> Word {
        Word::new("", Tense::Past, Body::none())
    }

    /// Whether this is the word that says what follows is now.
    pub fn is_now(&self) -> bool {
        self.kind.is_empty() && self.tense == Tense::Past && self.body.is_none()
    }

    /// A fact: a present word of a kind, which folds into the present and is logged.
    pub fn is_fact(&self) -> bool {
        self.tense == Tense::Present && is_kind(&self.kind)
    }
}

/// What a kind is, as town says when it is not one.
pub const KIND: &str = "a path, each part of it something, and none `.` or `..`";

/// Whether `kind` names one kind: a path with no part empty, `.` or `..`, as `nvim/` names what is
/// under it and `..` somewhere else. `nvim/1`, never `nvim/`, `/nvim`, `nvim//1` or `nvim/../x`.
pub fn is_kind(kind: &str) -> bool {
    kind.split('/').all(|part| !["", ".", ".."].contains(&part))
}

/// Where what hears `kind` stands, however it reads it: `nvim` for `nvim`, `nvim/` or `/nvim`, and
/// the root, nothing, for `/`.
pub fn place(kind: &str) -> &str {
    kind.trim_matches('/')
}

/// What failed. Every other word is said on purpose; an error is made by one before it, so errors
/// alone could go on without end, each making the next. It is heard only where it is named.
pub const ERROR: &str = "town/error";

/// Whether what hears `kind` hears a word of kind `said`, town's one rule: `nvim` is `nvim`;
/// `nvim/`, what is under it; `/nvim`, both; `/`, every kind. But `ERROR` only where it is named.
pub fn hears(kind: &str, said: &str) -> bool {
    if said == ERROR && place(kind) != ERROR {
        return false;
    }
    let (down, kind) = kind.strip_prefix('/').map_or((false, kind), |k| (true, k));
    let (under, kind) = kind.strip_suffix('/').map_or((false, kind), |k| (true, k));
    let below = kind.is_empty() || said.strip_prefix(kind).is_some_and(|r| r.starts_with('/'));
    (said == kind && !under) || ((down || under) && below)
}

/// Whether the path `kind` passes through `p`: `p`'s parts are in it whole, in order, anywhere, as
/// a Usenet site is in an article's `Path:`. `a/b/c` passes through `b` and `a/b`, not `a/c`.
pub fn through(p: &str, kind: &str) -> bool {
    let p: Vec<&str> = p.split('/').collect();
    let kind: Vec<&str> = kind.split('/').collect();
    kind.windows(p.len()).any(|parts| parts == p.as_slice())
}

/// Whether one that hears `kinds`, but `except`, hears a word of kind `said`: one of its kinds
/// names it (`hears`), and none of what it takes out does.
pub fn heard(kinds: &[String], except: &[String], said: &str) -> bool {
    kinds.iter().any(|k| hears(k, said)) && !except.iter().any(|k| hears(k, said))
}

#[cfg(test)]
mod tests {
    use super::{heard, hears, is_kind, through};

    // a path passes through another where the other's parts are in it whole and in order, at any
    // place: so a domain is found wherever it is, but never in part
    #[test]
    fn a_path_passes_through_what_is_in_it_whole() {
        assert!(through("b", "a/b/c") && through("a/b", "a/b/c") && through("a/b/c", "a/b/c"));
        assert!(!through("a/c", "a/b/c") && !through("b", "a/bb/c") && !through("a/b/c/d", "a/b"));
        assert!(!through("", "a"), "no kind has an empty part");
    }

    // what one takes out, `except`, it does not hear, of what its kinds name; and a kind is any
    // text
    #[test]
    fn an_exclusion_takes_out_what_it_names() {
        let kinds = |k: &[&str]| k.iter().map(|k| k.to_string()).collect::<Vec<_>>();
        let (all, other) = (kinds(&["/"]), kinds(&["other/"]));
        assert!(
            heard(&all, &other, "theme") && heard(&all, &other, "other"),
            "all but under other"
        );
        assert!(!heard(&all, &other, "other/theme") && !heard(&all, &other, "other/a/b"));
        let theme = kinds(&["theme"]);
        assert!(!heard(&theme, &kinds(&["/theme"]), "theme"), "taken out, however named");
        assert!(hears("!theme", "!theme"), "a kind is any text, `!` too");
    }

    // a kind is itself; with `/` after, what is under it; with `/` before, both; `/`, every kind
    #[test]
    fn a_kind_is_heard_as_a_path_is_read() {
        assert!(hears("nvim", "nvim") && !hears("nvim", "nvim/1") && !hears("nvim", "nvimx"));
        assert!(hears("nvim/", "nvim/1") && hears("nvim/", "nvim/a/b"));
        assert!(!hears("nvim/", "nvim") && !hears("nvim/", "nvimx"));
        assert!(hears("/nvim", "nvim") && hears("/nvim", "nvim/a/b") && !hears("/nvim", "nvimx"));
        assert!(!hears("/nvim/1", "nvim") && !hears("/nvim/1", "nvim/2"));
        assert!(hears("/", "note") && hears("/", "nvim/1") && hears("town/error", "town/error"));
        assert!(!hears("/", "town/error") && !hears("/town", "town/error"), "a failure is named");
        assert!(hears("/town/error", "town/error") && hears("/town", "town/residents/x"));
    }

    // a kind is a path that means only itself: no part of it empty, `.` or `..`
    #[test]
    fn a_kind_is_a_path_that_means_itself() {
        assert!(is_kind("nvim") && is_kind("nvim/1") && is_kind("a.b/c d"));
        for not in ["", "/", "nvim/", "/nvim", "nvim//1", "./nvim", "nvim/..", "a/../b"] {
            assert!(!is_kind(not), "{not:?}");
        }
    }
}
