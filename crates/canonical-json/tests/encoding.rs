use canonical_json::{canonical_bytes, canonical_hash, canonical_json};
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap};

#[test]
fn canonical_bytes_sorts_keys_and_keeps_number_spelling() {
    // The test build has preserve_order, so the parsed map keeps this order.
    let value: Value =
        serde_json::from_str(r#"{"b": 1.50, "a": {"d": [1, 10.0, -0], "c": "xé\n"}}"#).unwrap();
    assert_eq!(
        canonical_bytes("example", &value).unwrap(),
        b"example\0{\"a\":{\"c\":\"x\xc3\xa9\\n\",\"d\":[1,10.0,-0]},\"b\":1.50}".to_vec()
    );
}

#[test]
fn member_order_does_not_change_the_bytes() {
    let forward: Value = serde_json::from_str(r#"{"x": 1, "y": {"p": true, "q": null}}"#).unwrap();
    let reverse: Value = serde_json::from_str(r#"{"y": {"q": null, "p": true}, "x": 1}"#).unwrap();
    assert_eq!(
        canonical_bytes("t", &forward).unwrap(),
        canonical_bytes("t", &reverse).unwrap()
    );
    assert_eq!(
        canonical_json(&forward).unwrap(),
        br#"{"x":1,"y":{"p":true,"q":null}}"#
    );
}

#[test]
fn the_tag_separates_identities() {
    let value = json!({"id": "a"});
    assert_ne!(
        canonical_hash("polylogue-message", &value).unwrap(),
        canonical_hash("polylogue-document", &value).unwrap()
    );
    assert_eq!(
        canonical_hash("t", &value).unwrap(),
        blake3::hash(b"t\0{\"id\":\"a\"}")
    );
}

#[test]
fn keys_sort_by_utf8_bytes() {
    let value = json!({"b": 1, "B": 2, "\u{e9}": 3, "a": 4, "": 5});
    assert_eq!(
        canonical_json(&value).unwrap(),
        "{\"\":5,\"B\":2,\"a\":4,\"b\":1,\"\u{e9}\":3}".as_bytes()
    );
}

#[derive(Serialize)]
enum Shape {
    Unit,
    Wrapped(u8),
    Pair(i32, String),
    Fields { zeta: bool, alpha: Option<f64> },
}

#[derive(Serialize)]
struct Typed {
    second: Vec<Shape>,
    first: f64,
    whole: f64,
    absent: Option<u32>,
    numbered: BTreeMap<u32, &'static str>,
    hashed: HashMap<String, i64>,
    #[serde(flatten)]
    extra: BTreeMap<String, Value>,
    nested: Inner,
    character: char,
    unit: (),
}

#[derive(Serialize)]
struct Inner(u64);

#[test]
fn a_typed_value_and_its_value_form_give_the_same_bytes() {
    let typed = Typed {
        second: vec![
            Shape::Unit,
            Shape::Wrapped(7),
            Shape::Pair(-3, "p".into()),
            Shape::Fields {
                zeta: true,
                alpha: Some(0.25),
            },
        ],
        first: 1.5,
        whole: 2.0,
        absent: None,
        numbered: BTreeMap::from([(10, "ten"), (2, "two")]),
        hashed: HashMap::from([("z".into(), 1), ("m".into(), -1), ("a".into(), 0)]),
        extra: BTreeMap::from([("aa".into(), json!({"k": [1, {"y": 0, "x": 1}]}))]),
        nested: Inner(u64::MAX),
        character: 'c',
        unit: (),
    };
    let value = serde_json::to_value(&typed).unwrap();
    assert_eq!(
        canonical_bytes("t", &typed).unwrap(),
        canonical_bytes("t", &value).unwrap()
    );
    assert_eq!(
        String::from_utf8(canonical_json(&typed).unwrap()).unwrap(),
        concat!(
            r#"{"aa":{"k":[1,{"x":1,"y":0}]},"absent":null,"character":"c","first":1.5,"#,
            r#""hashed":{"a":0,"m":-1,"z":1},"nested":18446744073709551615,"#,
            r#""numbered":{"10":"ten","2":"two"},"#,
            r#""second":["Unit",{"Wrapped":7},{"Pair":[-3,"p"]},{"Fields":{"alpha":0.25,"zeta":true}}],"#,
            r#""unit":null,"whole":2.0}"#
        )
    );
}

#[test]
fn a_key_that_is_not_text_or_a_number_is_refused() {
    let map = BTreeMap::from([(vec![1u8], 1)]);
    assert!(canonical_bytes("t", &map).is_err());
}
