use proptest::prelude::*;
use query_language::{
    ErrorKind,
    cursor::*,
    identity::QuestionId,
    time::{Clock, DaySetting},
};
use std::{collections::BTreeMap, num::NonZeroUsize};

fn continuation() -> Continuation {
    Continuation {
        context: CursorContext {
            question: QuestionId([8; 32]),
            clock: Clock::new(
                "2026-03-29T00:00:00Z".parse().unwrap(),
                DaySetting::new(
                    jiff::tz::TimeZone::get("Europe/Warsaw").unwrap(),
                    jiff::civil::time(4, 0, 0, 0),
                )
                .unwrap(),
            ),
            resolved_times: vec![],
            random_seed: 47,
            read_positions: BTreeMap::from([("sx".into(), "commit:43".into())]),
            versions: BTreeMap::from([("grants".into(), "v3".into())]),
        },
        after: OrderPosition {
            values: vec![serde_json::json!(12), serde_json::Value::Null],
            row_identity: "row-c".into(),
        },
    }
}

#[test]
fn cursor_refusals() {
    let seal = CursorSeal::new([3; 32]);
    let data = continuation();
    let cursor = seal.seal(&data).unwrap();
    let opened = seal
        .open(&cursor, data.context.question, |context| {
            context.read_positions["sx"] == "commit:43"
        })
        .unwrap();
    assert_eq!(opened.after, data.after);
    assert_eq!(opened.context.clock, data.context.clock);
    assert_eq!(opened.context.random_seed, 47);
    assert_eq!(
        seal.open(&cursor, QuestionId([9; 32]), |_| true)
            .unwrap_err()
            .kind,
        ErrorKind::InvalidQuery
    );
    assert_eq!(
        seal.open(&cursor, data.context.question, |_| false)
            .unwrap_err()
            .kind,
        ErrorKind::StaleCursor
    );
    assert_eq!(
        CursorSeal::new([4; 32])
            .open(&cursor, data.context.question, |_| true)
            .unwrap_err()
            .kind,
        ErrorKind::InvalidQuery
    );
    let mut changed = cursor.as_str().as_bytes().to_vec();
    changed[10] = if changed[10] == b'0' { b'1' } else { b'0' };
    let changed = SealedCursor::from_wire(String::from_utf8(changed).unwrap());
    assert_eq!(
        seal.open(&changed, data.context.question, |_| panic!(
            "unverified payload reached authority"
        ))
        .unwrap_err()
        .kind,
        ErrorKind::InvalidQuery
    );
}

#[test]
fn page_is_not_the_answer_context() {
    let seal = CursorSeal::new([3; 32]);
    let data = continuation();
    let cursor = seal.seal(&data).unwrap();
    for size in [1, 2, 50, usize::MAX] {
        let request = PageRequest {
            page_size: NonZeroUsize::new(size).unwrap(),
            cursor: Some(cursor.clone()),
        };
        let context = seal
            .open(
                request.cursor.as_ref().unwrap(),
                data.context.question,
                |_| true,
            )
            .unwrap();
        assert_eq!(context.context.question, data.context.question);
        assert_eq!(context.after, data.after);
    }
}

proptest! {
    #[test]
    fn cursor_serialization_preserves_every_order_value(identity in ".*", values in prop::collection::vec(any::<i64>(),0..40), seed in any::<u64>()) {
        let mut data=continuation(); data.after.row_identity=identity;
        data.after.values=values.into_iter().map(serde_json::Value::from).collect(); data.context.random_seed=seed;
        let seal=CursorSeal::new([3;32]); let cursor=seal.seal(&data).unwrap();
        let decoded=seal.open(&cursor,data.context.question, |_|true).unwrap();
        prop_assert_eq!(decoded.after,data.after);prop_assert_eq!(decoded.context.random_seed,seed);
    }
    #[test]
    fn arbitrary_cursor_bytes_never_panic(text in ".*") {
        let _=CursorSeal::new([3;32]).open(&SealedCursor::from_wire(text),QuestionId([0;32]), |_|true);
    }
}

#[test]
fn cursor_keeps_rows_after_boundary_row_is_removed_and_new_rows_arrive() {
    use std::cmp::Ordering;
    let position = |key, id: &str| OrderPosition {
        values: vec![serde_json::json!(key)],
        row_identity: id.into(),
    };
    let first = vec![
        position(1, "a"),
        position(2, "b"),
        position(3, "c"),
        position(4, "d"),
    ];
    let after = first[1].clone();
    // The last row of page one is deleted. One row is inserted before the
    // boundary and one after it; unchanged rows c and d must still appear once.
    let mut changed = vec![
        position(1, "a"),
        position(1, "new-before"),
        position(3, "c"),
        position(3, "new-after"),
        position(4, "d"),
    ];
    changed.reverse();
    let directions = [Direction::Ascending];
    changed.retain(|p| compare_positions(p, &after, &directions).unwrap() == Ordering::Greater);
    changed.sort_by(|a, b| compare_positions(a, b, &directions).unwrap());
    assert_eq!(
        changed
            .iter()
            .map(|p| p.row_identity.as_str())
            .collect::<Vec<_>>(),
        vec!["c", "new-after", "d"]
    );
    assert_eq!(
        changed
            .iter()
            .filter(|p| ["c", "d"].contains(&p.row_identity.as_str()))
            .count(),
        2
    );
}

#[test]
fn keyset_unknowns_are_last_in_both_directions_and_numbers_remain_exact() {
    use std::cmp::Ordering;
    let position = |value| OrderPosition {
        values: vec![value],
        row_identity: "a".into(),
    };
    let unknown = position(serde_json::Value::Null);
    let small = position(serde_json::json!(9_007_199_254_740_992u64));
    let large = position(serde_json::json!(9_007_199_254_740_993u64));
    assert_eq!(
        compare_positions(&small, &large, &[Direction::Ascending]).unwrap(),
        Ordering::Less
    );
    assert_eq!(
        compare_positions(&small, &large, &[Direction::Descending]).unwrap(),
        Ordering::Greater
    );
    for direction in [Direction::Ascending, Direction::Descending] {
        assert_eq!(
            compare_positions(&unknown, &small, &[direction]).unwrap(),
            Ordering::Greater
        );
    }
    assert_eq!(
        compare_positions(
            &position(serde_json::json!(-0.5)),
            &position(serde_json::json!(-0.05)),
            &[Direction::Ascending]
        )
        .unwrap(),
        Ordering::Less
    );
    let mut tied = large.clone();
    tied.row_identity = "b".into();
    assert_eq!(
        compare_positions(&large, &tied, &[Direction::Descending]).unwrap(),
        Ordering::Less
    );
}

proptest! {
    #[test]
    fn decimal_key_order_agrees_with_independent_integer_order(a in any::<i64>(),b in any::<i64>()) {
        let position=|value|OrderPosition{values:vec![serde_json::json!(value)],row_identity:String::new()};
        prop_assert_eq!(compare_positions(&position(a),&position(b),&[Direction::Ascending]).unwrap(),a.cmp(&b));
    }
}
