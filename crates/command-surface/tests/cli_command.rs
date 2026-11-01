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
        OperationSpec {
            name: self.name.into(),
            command: self.command.iter().map(|word| word.to_string()).collect(),
            summary: "Record a note".into(),
            reason: "Notes have no existing operation".into(),
            http_route: format!("/v1/{}", self.command.join("/")),
            latency: LatencyClass::Read,
            failures: vec![],
            write: false,
        }
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
    }
}

#[test]
fn cli_command_output_is_unchanged_for_each_command_shape() {
    let table = table();
    let cases = [
        (
            request("read", "hello", "r-1", Deadline::None),
            "example read --request '{\"text\":\"hello\"}' --request-id r-1 --deadline none",
        ),
        (
            request("sink_add", "it's", "retry/1", Deadline::Milliseconds(250)),
            "example sink add --request '{\"text\":\"it'\\''s\"}' --request-id retry/1 --deadline 250",
        ),
        (
            request("read", "x", "r 2", Deadline::Default),
            "example read --request '{\"text\":\"x\"}' --request-id 'r 2'",
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
