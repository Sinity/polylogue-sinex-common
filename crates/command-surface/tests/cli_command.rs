use command_surface::*;
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Text {
    text: String,
}

/// Two operations with different command shapes: one word, and two words.
struct Note {
    name: &'static str,
    command: &'static [&'static str],
}

impl Operation for Note {
    type Request = Text;
    type Response = Text;
    fn spec(&self) -> OperationSpec {
        OperationSpec::request(
            self.name,
            self.command,
            "notes",
            "Record a note",
            "Notes have no existing operation",
        )
    }
    fn execute(&self, request: Text, _: RequestContext) -> OperationFuture<'_, Answer<Text>> {
        Box::pin(async move { Answer::ok(request) })
    }
    fn render_text(&self, answer: &Text) -> String {
        format!("{}\n", answer.text)
    }
}

fn table() -> OperationTable {
    let mut table = OperationTable::new("example", Deadlines::default());
    table
        .declare_tool(ToolFamily {
            name: "notes".into(),
            description: "Notes".into(),
            open_world: false,
        })
        .unwrap();
    table
        .register(Note {
            name: "read",
            command: &["read"],
        })
        .unwrap();
    table
        .register(Note {
            name: "sink_add",
            command: &["sink", "add"],
        })
        .unwrap();
    table
}

fn request(operation: &str, text: &str, request_id: &str, deadline: Deadline) -> RunnableRequest {
    RunnableRequest {
        operation: operation.into(),
        arguments: json!({ "text": text }),
        request_id: request_id.into(),
        deadline,
        budget_tokens: None,
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
