//! Property tests over the pure word + fold core — the invariants residents and the
//! present depend on. These generate thousands of random word sequences per run.
//!   cargo test --test fold_props

use std::collections::BTreeMap;

use proptest::prelude::*;
use serde_json::Value;
use town::town::fold;
use town::word::{Tense, Word};

fn arb_tense() -> impl Strategy<Value = Tense> {
    prop_oneof![Just(Tense::Past), Just(Tense::Present), Just(Tense::Future)]
}

// Bodies across the whole JSON space — null / bool / int / string / arrays / objects.
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

// Kinds: empty, plain, and arbitrary unicode (JSON-escaping stress).
fn arb_word() -> impl Strategy<Value = Word> {
    (
        prop_oneof![Just(String::new()), "[a-z]{1,6}", ".*"],
        arb_tense(),
        arb_body(),
    )
        .prop_map(|(kind, tense, body)| Word { kind, tense, body })
}

// An independent reference for the fold: last present, non-empty-kind word wins.
fn reference_fold(ws: &[Word]) -> BTreeMap<String, Value> {
    let mut m = BTreeMap::new();
    for w in ws {
        if w.tense == Tense::Present && !w.kind.is_empty() {
            m.insert(w.kind.clone(), w.body.clone());
        }
    }
    m
}

proptest! {
    // no input — valid or garbage — can panic the parser or the serializer.
    #[test]
    fn from_line_never_panics(s in ".*") {
        let _ = Word::from_line(&s);
    }
    #[test]
    fn to_line_never_panics(w in arb_word()) {
        let _ = w.to_line();
    }

    // from_line ∘ to_line is the identity on (kind, tense, body).
    #[test]
    fn roundtrip_is_identity(w in arb_word()) {
        let back = Word::from_line(&w.to_line()).expect("a serialized word must parse back");
        prop_assert_eq!(&back.kind, &w.kind);
        prop_assert_eq!(back.tense, w.tense);
        prop_assert_eq!(&back.body, &w.body);
    }

    // fold == the reference: last-wins per kind, over facts only (present + non-empty kind).
    #[test]
    fn fold_matches_reference(ws in prop::collection::vec(arb_word(), 0..64)) {
        prop_assert_eq!(fold(ws.clone().into_iter()), reference_fold(&ws));
    }

    // the invariant residents rely on: present == fold(all facts so far), at EVERY prefix.
    #[test]
    fn present_equals_fold_at_every_prefix(ws in prop::collection::vec(arb_word(), 0..48)) {
        let mut present: BTreeMap<String, Value> = BTreeMap::new();
        for i in 0..ws.len() {
            let w = &ws[i];
            if w.tense == Tense::Present && !w.kind.is_empty() {   // mirror Town::talk's insert
                present.insert(w.kind.clone(), w.body.clone());
            }
            prop_assert_eq!(&present, &fold(ws[..=i].iter().cloned()));
        }
    }

    // distinct kinds ⇒ the fold is order-independent (no cross-kind interference).
    #[test]
    fn distinct_kinds_fold_order_independently(
        mut ws in prop::collection::vec(arb_word(), 0..24),
        seed in any::<u64>(),
    ) {
        for (i, w) in ws.iter_mut().enumerate() {
            w.kind = format!("k{i}");   // force distinct, non-empty kinds
            w.tense = Tense::Present;
        }
        let folded = fold(ws.clone().into_iter());
        let mut rng = seed;                        // deterministic Fisher–Yates shuffle
        for i in (1..ws.len()).rev() {
            rng = rng.wrapping_mul(6364136223846793005).wrapping_add(1);
            ws.swap(i, (rng >> 33) as usize % (i + 1));
        }
        prop_assert_eq!(folded, fold(ws.into_iter()));
    }
}
