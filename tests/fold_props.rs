//! Properties of words and the fold, over many random words: what residents and the present rest
//! on. `cargo test --test fold_props`

use std::collections::BTreeMap;

use proptest::prelude::*;
use serde_json::Value;
use town::body::Body;
use town::testing;
use town::town::{fold, Town};
use town::word::{is_kind, Tense, Word};

fn arb_tense() -> impl Strategy<Value = Tense> {
    prop_oneof![Just(Tense::Past), Just(Tense::Present), Just(Tense::Future)]
}

// a body of any JSON: null, bool, number, text, list or map
fn arb_body() -> impl Strategy<Value = Value> {
    let leaf = prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::from),
        any::<i64>().prop_map(Value::from),
        ".*".prop_map(Value::from),
    ];
    leaf.prop_recursive(3, 16, 5, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..5).prop_map(Value::from),
            prop::collection::hash_map("[a-z]{1,4}", inner, 0..5)
                .prop_map(|m| Value::Object(m.into_iter().collect())),
        ]
    })
}

// a word of any tense and body, its kind empty, plain, a path, or any text, to be escaped
fn arb_word() -> impl Strategy<Value = Word> {
    (prop_oneof![Just(String::new()), "[a-z]{1,6}", "[a-z./]{1,6}", ".*"], arb_tense(), arb_body())
        .prop_map(|(kind, tense, body)| Word::new(kind, tense, Body::of(body)))
}

// the fold, apart from town's: the last present word of a kind wins, and one with no body takes
// its kind away; a word of no kind, `""` or `a//b`, is none of the present
fn reference_fold(ws: &[Word]) -> BTreeMap<String, Body> {
    let mut m = BTreeMap::new();
    for w in ws {
        if w.tense == Tense::Present && is_kind(&w.kind) {
            if w.body.is_none() {
                m.remove(&w.kind);
            } else {
                m.insert(w.kind.clone(), w.body.clone());
            }
        }
    }
    m
}

proptest! {
    // no line, a word or not, panics the reader
    #[test]
    fn reading_never_panics(s in ".*") {
        let _ = town::voice::read(&s);
    }

    // a word written as a line reads back as itself: its kind, tense and body
    #[test]
    fn a_word_reads_back_as_itself(w in arb_word()) {
        let back = town::voice::read(&town::voice::line(&w));
        let back = back.expect("a word said is read back");
        prop_assert_eq!(&back.kind, &w.kind);
        prop_assert_eq!(back.tense, w.tense);
        prop_assert_eq!(&back.body, &w.body);
    }

    // town's fold is the reference fold, of any words
    #[test]
    fn fold_matches_reference(ws in prop::collection::vec(arb_word(), 0..64)) {
        prop_assert_eq!(fold(ws.clone().into_iter()), reference_fold(&ws));
    }

    // the present a town keeps as it talks is the fold of every word so far, after each word; and
    // a town rebuilt from its log keeps the same
    #[test]
    fn present_equals_fold_at_every_prefix(ws in prop::collection::vec(arb_word(), 0..48)) {
        let dir = testing::scratch();
        let town = Town::with_log(dir.log());
        for (i, w) in ws.iter().enumerate() {
            town.talk(w.clone());
            prop_assert_eq!(&*town.present.borrow(), &fold(ws[..=i].iter().cloned()));
        }
        let rebuilt = Town::with_log(dir.log());
        prop_assert_eq!(&*rebuilt.present.borrow(), &*town.present.borrow());
    }
}
