use authored::{OrderError, OrderKey, SortKey};
use proptest::prelude::*;

/// A small deterministic generator for the simulated writers.
struct Random(u64);

impl Random {
    fn next(&mut self) -> u64 {
        // xorshift64*
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }
    fn below(&mut self, bound: usize) -> usize {
        (self.next() % bound as u64) as usize
    }
}

#[test]
fn order_stable() {
    // Two writers insert 1,000 times into gaps of one shared list, often the
    // same gap from the same snapshot. Every key lands strictly inside its
    // gap, no existing key changes, and the list stays a total order.
    let mut random = Random(0x9e37_79b9_7f4a_7c15);
    let mut list: Vec<SortKey> = Vec::new();
    let mut inserted = 0;
    let mut writer = 0u32;
    while inserted < 1_000 {
        let gap = random.below(list.len() + 1);
        let before = gap.checked_sub(1).map(|index| list[index].key.clone());
        let after = list.get(gap).map(|entry| entry.key.clone());
        let writers = if random.below(3) == 0 { 2 } else { 1 };
        let snapshot = list.clone();
        let mut new = Vec::new();
        for _ in 0..writers {
            let key =
                OrderKey::between(before.as_ref(), after.as_ref(), random.next() as u32).unwrap();
            if let Some(before) = &before {
                assert!(&key > before);
            }
            if let Some(after) = &after {
                assert!(&key < after);
            }
            writer += 1;
            new.push(SortKey {
                key,
                root: format!("slot{writer}"),
            });
        }
        list.extend(new);
        list.sort();
        inserted += writers;
        // Nothing that existed was rewritten.
        assert!(snapshot.iter().all(|entry| list.contains(entry)));
    }
    assert!(list.windows(2).all(|pair| pair[0] < pair[1]));
    let mut keys: Vec<_> = list.iter().map(|entry| &entry.key).collect();
    keys.dedup();
    assert_eq!(keys.len(), list.len(), "two writers produced one key");
}

#[test]
fn order_dense() {
    // 10,000 successive insertions between the same two neighbours, on each
    // side, give distinct ordered keys: no precision runs out.
    for toward_left in [true, false] {
        let left = OrderKey::parse("A").unwrap();
        let right = OrderKey::parse("B").unwrap();
        let mut newest: Option<OrderKey> = None;
        let mut keys = Vec::new();
        for n in 0..10_000u32 {
            let key = if toward_left {
                OrderKey::between(Some(&left), newest.as_ref().or(Some(&right)), n)
            } else {
                OrderKey::between(newest.as_ref().or(Some(&left)), Some(&right), n)
            }
            .unwrap();
            assert!(key > left && key < right);
            newest = Some(key.clone());
            keys.push(key);
        }
        if toward_left {
            keys.reverse();
        }
        assert!(keys.windows(2).all(|pair| pair[0] < pair[1]));
    }
}

#[test]
fn order_equal_keys_by_root() {
    let key = OrderKey::parse("V").unwrap();
    let mut sort_keys = [
        SortKey {
            key: key.clone(),
            root: "b".into(),
        },
        SortKey {
            key: OrderKey::parse("U").unwrap(),
            root: "z".into(),
        },
        SortKey {
            key,
            root: "a".into(),
        },
    ];
    sort_keys.sort();
    let roots: Vec<_> = sort_keys
        .iter()
        .map(|sort_key| sort_key.root.as_str())
        .collect();
    assert_eq!(roots, ["z", "a", "b"]);
}

#[test]
fn order_keys_are_checked() {
    assert_eq!(OrderKey::parse(""), Err(OrderError::Empty));
    assert!(matches!(OrderKey::parse("a-b"), Err(OrderError::Digit(_))));
    assert!(matches!(
        OrderKey::parse("V0"),
        Err(OrderError::TrailingZero(_))
    ));
    let (a, b) = (OrderKey::parse("a").unwrap(), OrderKey::parse("b").unwrap());
    assert!(matches!(
        OrderKey::between(Some(&b), Some(&a), 1),
        Err(OrderError::NotBefore { .. })
    ));
    assert!(OrderKey::between(Some(&a), Some(&a), 1).is_err());
    let json = serde_json::to_string(&a).unwrap();
    assert_eq!(json, "\"a\"");
    assert_eq!(serde_json::from_str::<OrderKey>(&json).unwrap(), a);
    assert!(serde_json::from_str::<OrderKey>("\"a0\"").is_err());
}

#[test]
fn order_evenly() {
    for count in [0usize, 1, 2, 61, 62, 3_843, 3_844, 10_000] {
        let keys = OrderKey::evenly(count);
        assert_eq!(keys.len(), count);
        assert!(keys.windows(2).all(|pair| pair[0] < pair[1]));
        for key in &keys {
            assert_eq!(OrderKey::parse(key.as_str()).as_ref(), Ok(key));
        }
    }
}

fn key() -> impl Strategy<Value = OrderKey> {
    "[0-9A-Za-z]{0,6}[1-9A-Za-z]".prop_map(|text| OrderKey::parse(&text).unwrap())
}

proptest! {
    #[test]
    fn order_between_any_two_keys(a in key(), b in key(), suffix in any::<u32>()) {
        prop_assume!(a != b);
        let (low, high) = if a < b { (a, b) } else { (b, a) };
        let key = OrderKey::between(Some(&low), Some(&high), suffix).unwrap();
        prop_assert!(low < key && key < high);
        let first = OrderKey::between(None, Some(&low), suffix).unwrap();
        prop_assert!(first < low);
        let last = OrderKey::between(Some(&high), None, suffix).unwrap();
        prop_assert!(high < last);
        // The result is a valid key.
        prop_assert_eq!(OrderKey::parse(key.as_str()), Ok(key));
    }
}
