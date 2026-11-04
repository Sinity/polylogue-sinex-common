use super::*;
use crate::catalog::Function;

#[test]
fn function_lookup_folds_case_and_refuses_case_collisions() {
    let mut catalog = Catalog::new("rows");
    assert_eq!(catalog.function("DAY").unwrap().unwrap().name, "day");
    assert!(catalog.function("custom").unwrap().is_none());
    catalog.functions.push(Function {
        name: "Day".into(),
        arguments: 1,
        result: FieldKind::Text,
        help: String::new(),
    });
    assert_eq!(
        catalog.function("day").unwrap_err().kind,
        ErrorKind::Ambiguous
    );
}

#[test]
fn near_misses_are_ordered_and_short_words_never_match() {
    let names = ["sort", "short", "sorts"];
    assert_eq!(closest("srot", names.into_iter()), ["sort"]);
    assert!(closest("srt", names.into_iter()).is_empty());
}
