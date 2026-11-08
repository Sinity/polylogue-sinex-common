use command_surface::*;
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Text {
    text: String,
}

fn table() -> std::sync::Arc<OperationCatalog> {
    let mut catalog = OperationCatalog::builder("example", Deadlines::default());
    catalog
        .declare_tool(ToolFamily {
            name: "notes".into(),
            description: "Notes".into(),
            open_world: false,
            idempotent: true,
        })
        .unwrap();
    for (name, command) in [("read", &["read"][..]), ("sink_add", &["sink", "add"][..])] {
        catalog
            .declare::<Text, Text>(
                OperationSpec::request(
                    name,
                    command,
                    "notes",
                    "Record a note",
                    "Notes have no existing operation",
                ),
                |answer, _style| format!("{}\n", answer.text),
            )
            .unwrap();
    }
    catalog.finish()
}

fn request(operation: &str, text: &str, request_id: &str, deadline: Deadline) -> RunnableRequest {
    RunnableRequest {
        operation: operation.into(),
        arguments: json!({ "text": text }),
        request_id: request_id.into(),
        deadline,
        budget_tokens: command_surface::TokenBudget::Default,
        cursor: None,
        fresh: false,
        format: Format::Text,
    }
}

#[test]
fn cli_command_writes_each_command_shape_in_its_own_flags() {
    let table = table();
    let cases = [
        (
            request("read", "hello", "r-1", Deadline::None),
            "example read --text hello --request-id r-1 --deadline none",
        ),
        (
            request("sink_add", "it's", "retry/1", Deadline::Milliseconds(250)),
            "example sink add --text 'it'\\''s' --request-id retry/1 --deadline 250",
        ),
        (
            request("read", "x", "r 2", Deadline::Default),
            "example read --text x --request-id 'r 2'",
        ),
    ];
    for (request, expected) in cases {
        assert_eq!(table.cli_command(&request).unwrap(), expected);
    }
}

#[test]
fn cli_command_refuses_an_unknown_operation() {
    let table = table();
    let unknown = request("missing", "x", "r-1", Deadline::None);
    assert!(table.cli_command(&unknown).is_err());
}

#[test]
fn cli_command_round_trips_through_the_parser() {
    let table = table();
    for request in [
        request("read", "hello", "r-1", Deadline::None),
        request("sink_add", "--json", "retry/1", Deadline::Milliseconds(250)),
        request("read", "", "r 2", Deadline::Default),
    ] {
        let command = table.cli_command(&request).unwrap();
        let words = shell_words(&command);
        assert_eq!(words[0], "example");
        let parsed = table.parse_cli(&words[1..], "unused").unwrap();
        assert_eq!(parsed.request, request, "{command}");
    }
}

#[test]
fn displayed_cli_commands_keep_controls_in_round_tripping_json() {
    let table = table();
    let controls = (0..=0x9f)
        .filter_map(char::from_u32)
        .filter(|c| c.is_control())
        .chain([
            '\u{061c}', '\u{200e}', '\u{200f}', '\u{202a}', '\u{202b}', '\u{202c}', '\u{202d}',
            '\u{202e}', '\u{2066}', '\u{2067}', '\u{2068}', '\u{2069}',
        ]);
    for c in controls {
        for extra in [false, true] {
            let mut request = request(
                "read",
                &format!("it's {c} literal \\x9b and \\u009b"),
                "r-1",
                Deadline::Milliseconds(250),
            );
            // An undeclared field already needs the whole-JSON form; controls
            // in natural flags must choose the same lossless representation.
            if extra {
                request.arguments["extra"] = json!(["value", {"control":c.to_string()}]);
            }
            let command = table.cli_command(&request).unwrap();
            assert!(command.contains(" --request "), "{command:?}");
            assert!(!command.contains(c), "{command:?}");
            let words = shell_words(&command);
            let parsed = table.parse_cli(&words[1..], "unused").unwrap();
            assert_eq!(parsed.request, request, "{command:?}");
        }
    }
}

/// Split a command line the way `sh` does for the quoting `quote_argument` writes.
fn shell_words(command: &str) -> Vec<String> {
    let output = std::process::Command::new("sh")
        .args(["-c", &format!("printf '%s\\0' {command}")])
        .output()
        .unwrap();
    assert!(output.status.success());
    // Each word ends with a NUL; an empty word stays a word.
    output
        .stdout
        .strip_suffix(&[0])
        .unwrap()
        .split(|byte| *byte == 0)
        .map(|word| String::from_utf8(word.to_vec()).unwrap())
        .collect()
}
