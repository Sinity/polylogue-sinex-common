use super::*;
use crate::{ArgumentSpec, FailureKind};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Note {
    text: String,
}
#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Save {
    name: Option<String>,
}
#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Read {
    view: Option<String>,
    #[serde(default)]
    follow: bool,
}

fn declarations() -> StepDeclarations {
    let mut declarations = StepDeclarations::default();
    declarations
        .declare::<Note>(
            "note",
            vec![ArgumentSpec::new("text", ArgumentForm::Positional, "text")],
        )
        .unwrap();
    declarations
        .declare::<Save>(
            "save",
            vec![ArgumentSpec::new("name", ArgumentForm::Positional, "name")],
        )
        .unwrap();
    declarations
        .declare::<Read>(
            "read",
            vec![
                ArgumentSpec::new("view", ArgumentForm::Named, "view"),
                ArgumentSpec::new("follow", ArgumentForm::Flag, "follow"),
            ],
        )
        .unwrap();
    declarations
}
fn words(words: &[&str]) -> Vec<String> {
    words.iter().map(|word| (*word).into()).collect()
}
fn split(words: &[&str]) -> Result<SplitStep, SplitError<Infallible>> {
    declarations().split_step(&self::words(words), 0, |_, _, _| Ok(false))
}

#[test]
fn required_and_optional_positionals_keep_verb_words_distinct() {
    let note = split(&["note", "read", "read"]).unwrap();
    assert_eq!(note.end, 2);
    assert_eq!(note.step, json!({"verb":"note", "text":"read"}));
    let save = split(&["save", "read"]).unwrap();
    assert_eq!(save.end, 1);
    assert_eq!(save.step, json!({"verb":"save"}));
    let save = split(&["save", "named", "read"]).unwrap();
    assert_eq!(save.end, 2);
    assert_eq!(save.step, json!({"verb":"save", "name":"named"}));
}

#[test]
fn literal_positionals_and_option_values_keep_their_original_positions() {
    for literal in ["read", "--follow", "-", "--"] {
        let parsed = split(&["note", "--", literal, "read"]).unwrap();
        assert_eq!(parsed.end, 3);
        assert_eq!(parsed.step, json!({"verb":"note", "text":literal}));
        assert_eq!(
            parsed.argument_words,
            [ArgumentWord {
                field: "text".into(),
                index: 2,
                positional: true
            }]
        );
    }
    let read = split(&["read", "--view", "note", "--follow", "save"]).unwrap();
    assert_eq!(read.end, 4);
    assert_eq!(
        read.step,
        json!({"verb":"read", "view":"note", "follow":true})
    );
    assert_eq!(
        read.argument_words,
        [ArgumentWord {
            field: "view".into(),
            index: 2,
            positional: false
        }]
    );
}

#[test]
fn product_options_are_consumed_without_changing_typed_fields() {
    let args = words(&["note", "--json", "literal text", "--to", "out", "read"]);
    let mut local = Vec::new();
    let parsed = declarations()
        .split_step(&args, 0, |verb, args, index| {
            assert_eq!(verb, "note");
            match args[*index].as_str() {
                "--json" => local.push("json".to_owned()),
                "--to" => {
                    *index += 1;
                    local.push(args[*index].clone());
                }
                _ => return Ok::<_, Infallible>(false),
            }
            Ok(true)
        })
        .unwrap();
    assert_eq!(parsed.end, 5);
    assert_eq!(parsed.step, json!({"verb":"note", "text":"literal text"}));
    assert_eq!(local, ["json", "out"]);
    assert_eq!(parsed.argument_words[0].index, 2);
}

#[test]
fn structural_refusals_keep_the_deciding_word() {
    assert!(
        matches!(split(&["note"]), Err(SplitError::MissingArgument { index:1, name }) if name == "text")
    );
    assert!(matches!(
        split(&["read", "--view"]),
        Err(SplitError::MissingValue { index: 1 })
    ));
    assert!(matches!(
        split(&["read", "--unknown"]),
        Err(SplitError::UnknownOption { index: 1 })
    ));
    assert!(matches!(
        split(&["unknown"]),
        Err(SplitError::UnknownVerb { index: 0 })
    ));
    assert!(matches!(
        split(&["read", "--follow", "--follow"]),
        Err(SplitError::Arguments(Failure {
            kind: FailureKind::BadRequest,
            ..
        }))
    ));
}

struct Rules;
impl SelectionRules for Rules {
    type Additional = &'static str;
    fn is_command(&self, word: &str) -> bool {
        word == "status"
    }
    fn saved_name(&self, word: &str) -> Option<String> {
        ["status", "read", "mine", "events"]
            .contains(&word)
            .then(|| word.into())
    }
    fn is_unit(&self, word: &str) -> bool {
        ["events", "materials"].contains(&word)
    }
    fn is_row_numbers(&self, word: &str) -> bool {
        word == "@1"
    }
    fn is_reference(&self, word: &str) -> bool {
        ["@1", "exact"].contains(&word)
    }
    fn additional(&self, word: &str) -> Option<Self::Additional> {
        match word {
            "yesterday" => Some("time"),
            "/some/file" => Some("path"),
            _ => None,
        }
    }
    fn is_query(&self, word: &str) -> bool {
        word.chars().any(|c| c.is_whitespace() || c == '|')
    }
}
#[test]
fn selection_precedence_uses_product_catalogs_before_fallbacks() {
    let declarations = declarations();
    for (word, expected) in [
        (None, FirstWord::Nothing),
        (Some("-"), FirstWord::Stdin),
        (Some("status"), FirstWord::Command),
        (Some("read"), FirstWord::Verb),
        (
            Some("mine"),
            FirstWord::SavedQuery {
                name: "mine".into(),
            },
        ),
        (
            Some("events"),
            FirstWord::SavedQuery {
                name: "events".into(),
            },
        ),
        (Some("materials"), FirstWord::Unit),
        (Some("@1"), FirstWord::RowNumbers),
        (Some("exact"), FirstWord::Reference),
        (Some("yesterday"), FirstWord::Additional("time")),
        (Some("/some/file"), FirstWord::Additional("path")),
        (Some("yesterday | where kind = x"), FirstWord::QueryText),
        (Some("bare"), FirstWord::SearchWord),
    ] {
        assert_eq!(classify_first(word, &Rules, &declarations), expected);
    }
}
