//! Any text through parsing, binding, formatting, identity, copying and
//! serialization: each answers or refuses, and nothing crashes.
#![no_main]
use libfuzzer_sys::fuzz_target;
use query_language::{bind::bind, format::format_query, identity::normal_form, parse::parse};

#[path = "../../crates/query-language/tests/support/mod.rs"]
mod support;

fuzz_target!(|bytes: &[u8]| {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return;
    };
    let catalog = support::polylogue();
    let Ok(query) = parse(text, &catalog) else {
        return;
    };
    assert_eq!(query.clone(), query);
    let _ = serde_json::to_string(&query).unwrap();
    let _ = bind(&query, &catalog);
    if let Ok(rendered) = format_query(&query, &catalog) {
        let _ = parse(&rendered, &catalog);
    }
    let _ = normal_form(&query, &catalog);
});
