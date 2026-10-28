use command_surface::{Availability, Deadline, LatencyClass, OperationSpec, Request, SseDecoder};
use http::{Method, Request as HttpRequest, Response, StatusCode};
use serde_json::json;
use web_shell::protocol::{Action, CopyForm, CopyTarget};
use web_shell::{FragmentUpdate, Frame, Product, ValueState, html, value_cell};

#[test]
fn live_fragment_equals_reload() {
    struct Answer {
        text: String,
    }
    let answer = Answer {
        text: "<script>captured text</script>".into(),
    };
    let view = |answer: &Answer| html! { article id="row" data-row="" { (answer.text) } };
    let fragment = FragmentUpdate::replace("rows", &answer, &view);
    let FragmentUpdate::Replace { html: fragment, .. } = fragment else {
        unreachable!()
    };
    let page = Frame::new(Product::Polylogue, "Reader", "/find", "app read row")
        .render(&answer, &view)
        .into_string();
    assert!(page.contains(&fragment));
    assert!(fragment.contains("&lt;script&gt;"));
    assert!(!page.contains("<script>captured"));
    assert!(!page.contains(" style="));
    assert!(!page.contains("<script>"));
}

#[test]
fn unknown_never_zero() {
    let states: Vec<ValueState<u64>> = vec![
        ValueState::Unknown { reason: None },
        ValueState::NotRecorded,
        ValueState::NotApplicable,
        ValueState::Pending,
        ValueState::Error {
            reason: "source offline".into(),
        },
    ];
    for state in states {
        let cell = value_cell(&state).into_string();
        assert!(!cell.contains('0'), "{cell}");
    }
    assert!(
        value_cell(&ValueState::Measured(0))
            .into_string()
            .contains(">0<")
    );
    assert!(
        value_cell(&ValueState::Partial {
            value: 3,
            unknown: 2
        })
        .into_string()
        .contains("≥ 3 (2 unknown)")
    );
    assert!(
        value_cell(&ValueState::Inferred(4))
            .into_string()
            .contains("≈ 4 (inferred)")
    );
}

#[test]
fn csp_on_every_route() {
    for method in [Method::GET, Method::HEAD, Method::POST] {
        for path in [
            "/assets/tokens.css",
            "/assets/shell.js",
            "/assets/shell.css",
            "/assets/fonts/IBMPlexSans-Regular.woff2",
            "/assets/fonts/IBMPlexMono-Regular.woff2",
            "/assets/theme.css",
            "/assets/../secret",
        ] {
            let request = HttpRequest::builder()
                .method(method.clone())
                .uri(path)
                .body(())
                .unwrap();
            let response = web_shell::assets::serve(&request);
            assert_eq!(
                response.headers()["content-security-policy"],
                web_shell::assets::CONTENT_SECURITY_POLICY
            );
            assert_eq!(response.headers()["x-content-type-options"], "nosniff");
            if method == Method::HEAD {
                assert!(response.body().is_empty());
            }
            if method == Method::POST {
                assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
            }
        }
    }
    let response = web_shell::assets::secure(Response::builder().status(500).body(()).unwrap());
    assert_eq!(
        response.headers()["content-security-policy"],
        web_shell::assets::CONTENT_SECURITY_POLICY
    );
}

#[test]
fn assets_validate_cache_and_font_identity() {
    let request = HttpRequest::builder()
        .uri("/assets/shell.js")
        .body(())
        .unwrap();
    let response = web_shell::assets::serve(&request);
    assert_eq!(response.status(), StatusCode::OK);
    assert!(!response.body().is_empty());
    let etag = response.headers()["etag"].to_str().unwrap();
    let conditional = HttpRequest::builder()
        .uri("/assets/shell.js")
        .header("if-none-match", etag)
        .body(())
        .unwrap();
    let response = web_shell::assets::serve(&conditional);
    assert_eq!(response.status(), StatusCode::NOT_MODIFIED);
    assert!(response.body().is_empty());
    for family in ["Sans", "Mono"] {
        let request = HttpRequest::builder()
            .uri(format!("/assets/fonts/IBMPlex{family}-Regular.woff2"))
            .body(())
            .unwrap();
        let font = web_shell::assets::serve(&request);
        assert_eq!(&font.body()[..4], b"wOF2");
        assert_eq!(font.headers()["content-type"], "font/woff2");
    }
}

#[test]
fn fragment_event_round_trips_through_shared_sse_decoder() {
    let update = FragmentUpdate::Append {
        target: "reader".into(),
        html: "<p>line\nλ</p>".into(),
    };
    let event = update.event("position-3").unwrap();
    let mut decoder = SseDecoder::default();
    let events = decoder.push(event.as_bytes()).unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].id.as_deref(), Some("position-3"));
    assert_eq!(events[0].event, "fragment");
    assert_eq!(
        serde_json::from_str::<FragmentUpdate>(&events[0].data).unwrap(),
        update
    );
    assert!(update.event("position\ninjected: yes").is_err());
}

fn operation(write: bool) -> OperationSpec {
    OperationSpec {
        name: "read".into(),
        command: vec!["read".into()],
        summary: "Read selected row".into(),
        reason: "read one row".into(),
        http_route: "/v1/read".into(),
        latency: LatencyClass::Read,
        failures: vec![],
        write,
    }
}
#[test]
fn actions_are_operation_requests_and_keep_unavailable_reasons() {
    let request = Request {
        operation: "read".into(),
        arguments: json!({"ref": "row & <one>"}),
        request_id: "request-1".into(),
        deadline: Deadline::Default,
    };
    let action = Action::from_operation(
        &operation(true),
        &Availability::Disabled {
            reason: "source unavailable".into(),
        },
        &request,
        "app read row".into(),
        None,
    )
    .unwrap();
    let markup = action.render().into_string();
    assert!(markup.contains("method=\"post\""));
    assert!(markup.contains("name=\"request\""));
    assert!(markup.contains("disabled"));
    assert!(markup.contains("source unavailable"));
    let read = Action::from_operation(
        &operation(false),
        &Availability::Enabled,
        &request,
        "app read row".into(),
        None,
    )
    .unwrap()
    .render()
    .into_string();
    assert!(read.contains("href=\"/v1/read?ref=row%20%26%20%3Cone%3E\""));
    let mut wrong = request.clone();
    wrong.operation = "different".into();
    assert!(
        Action::from_operation(
            &operation(false),
            &Availability::Enabled,
            &wrong,
            "".into(),
            None
        )
        .is_err()
    );
    assert!(
        Action::from_operation(
            &operation(false),
            &Availability::Enabled,
            &request,
            "".into(),
            Some("j".into())
        )
        .is_err()
    );
}

#[test]
fn copy_target_escapes_text_and_round_trips_forms() {
    let target = CopyTarget {
        label: "message".into(),
        forms: vec![CopyForm {
            label: "Markdown".into(),
            text: "\"<script>&🦋".into(),
        }],
    };
    let markup = target.render(html! { p { "body" } }).into_string();
    assert!(!markup.contains("<script>"));
    assert!(markup.contains("data-copy-feedback"));
    assert_eq!(
        serde_json::from_str::<CopyTarget>(&serde_json::to_string(&target).unwrap()).unwrap(),
        target
    );
}

#[test]
fn generated_types_match_rust() {
    assert_eq!(
        web_shell::protocol::typescript(),
        include_str!("../typescript/types.ts")
    );
}

#[test]
fn same_key_same_act() {
    use web_shell::keys::KeyAction;
    for (letter, terminal, expected_action) in [
        ("j", "Ctrl-j", KeyAction::Next),
        ("k", "Ctrl-k", KeyAction::Previous),
        ("y", "Ctrl-y", KeyAction::CopyMenu),
        ("x", "Ctrl-x", KeyAction::ToggleSelection),
        ("n", "Ctrl-n", KeyAction::NextMatch),
    ] {
        let binding = web_shell::keys::KEYS
            .iter()
            .find(|binding| binding.key == letter && !binding.ctrl)
            .unwrap();
        assert_eq!(binding.action, expected_action);
        assert!(
            web_shell::keys::picker_bindings()
                .any(|(key, action)| key == terminal && action == expected_action)
        );
    }
}

#[test]
fn palette_contrast_and_css_injection() {
    use web_shell::theme::Palette;
    assert!(Palette::new("#ffffff", "#ffffff", "#eeeeee", "#777777").is_err());
    assert!(Palette::new("#ffffff;url(x)", "#ffffff", "#eeeeee", "#000000").is_err());
    let palette = Palette::pywal(r##"{"special":{"background":"#101010","foreground":"#eeeeee"},"colors":{"color0":"#202020"}}"##).unwrap();
    let css = palette.stylesheet();
    assert!(css.contains("--ground:#101010"));
    assert!(!css.contains("failure-ink"));
}

#[test]
fn palette_and_picker_share_matcher_vectors() {
    let vectors: Vec<serde_json::Value> =
        serde_json::from_str(include_str!("matcher.json")).unwrap();
    for vector in vectors {
        let score = web_shell::matching::match_score(
            vector["query"].as_str().unwrap(),
            vector["label"].as_str().unwrap(),
        );
        assert_eq!(score, vector["score"].as_f64(), "{vector}");
    }
}

#[test]
fn page_and_palette_cli_derive_from_registered_request() {
    struct Read;
    impl command_surface::Operation for Read {
        type Request = serde_json::Value;
        type Response = serde_json::Value;
        fn spec(&self) -> OperationSpec {
            operation(false)
        }
        fn execute(
            &self,
            request: Self::Request,
            _: command_surface::RequestContext,
        ) -> command_surface::OperationFuture<'_, command_surface::Answer<Self::Response>> {
            Box::pin(async move { command_surface::Answer::ok(request) })
        }
        fn render_text(&self, value: &Self::Response) -> String {
            value.to_string()
        }
    }
    let mut table =
        command_surface::OperationTable::new("app", command_surface::Deadlines::default());
    table.register(Read).unwrap();
    let request = Request {
        operation: "read".into(),
        arguments: json!({"ref":"row' & λ"}),
        request_id: "r-1".into(),
        deadline: Deadline::None,
    };
    let action = Action::from_table(&table, &Availability::Enabled, &request, None).unwrap();
    assert_eq!(action.cli, table.cli_command(&request).unwrap());
    let frame = Frame::for_request(Product::Sinex, "Reader", "/find", &table, &request).unwrap();
    assert_eq!(frame.command, action.cli);
    let markup = frame
        .render(&(), &|_: &()| html! { p { "row" } })
        .into_string();
    assert!(markup.contains("page-command"));
    assert!(action.cli.contains("--deadline none"));
}
