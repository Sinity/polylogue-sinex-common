//! Cards, seen marks, the frame's notices and counterpart, the key table and
//! its product checks, and the no-script document under every element.
use command_surface::{Access, Availability, Deadline, OperationSpec, Request};
use serde_json::json;
use web_shell::card::CardError;
use web_shell::keys::{self, KEYS, KeyAction, KeyConflict, Place, ProductKey};
use web_shell::protocol::Action;
use web_shell::{Card, CardAction, Counterpart, Feed, Frame, Product, Seen, html};

fn action(name: &str, write: bool) -> Action {
    let spec = OperationSpec {
        access: if write { Access::Write } else { Access::Read },
        ..OperationSpec::request(name, &[name], name, format!("Do {name}"), "test")
    };
    let request = Request {
        operation: name.into(),
        arguments: json!({"target": "session:a"}),
        request_id: format!("{name}-1"),
        deadline: Deadline::Default,
        budget_tokens: command_surface::TokenBudget::Default,
        cursor: None,
        fresh: false,
        format: command_surface::Format::Text,
    };
    Action::from_operation(
        &spec,
        &Availability::Enabled,
        &request,
        format!("app {name}"),
        None,
    )
    .unwrap()
}

#[test]
fn card_keys_are_shown_and_each_action_is_a_form() {
    let card = Card::new(
        "Permission: Bash",
        html! { p { "rm -r build" } },
        vec![
            CardAction {
                key: 'a',
                action: action("approve", true),
            },
            CardAction {
                key: 'd',
                action: action("deny", true),
            },
        ],
    )
    .unwrap()
    .render()
    .into_string();
    assert!(card.starts_with("<shell-card tabindex=\"-1\""));
    assert_eq!(card.matches("<form method=\"post\"").count(), 2);
    assert!(card.contains("data-card-key=\"a\""));
    assert!(card.contains("<kbd>a</kbd> Do approve"));
    assert!(card.contains("Enter to use these keys"));
}

#[test]
fn cards_refuse_ambiguous_or_unwritable_actions() {
    let twice = Card::new(
        "x",
        html! {},
        vec![
            CardAction {
                key: 'a',
                action: action("approve", true),
            },
            CardAction {
                key: 'a',
                action: action("again", true),
            },
        ],
    );
    assert_eq!(twice.err(), Some(CardError::KeyTwice('a')));
    let read = Card::new(
        "x",
        html! {},
        vec![CardAction {
            key: 'a',
            action: action("read", false),
        }],
    );
    assert_eq!(read.err(), Some(CardError::NotACardAction("read".into())));
    let space = Card::new(
        "x",
        html! {},
        vec![CardAction {
            key: ' ',
            action: action("approve", true),
        }],
    );
    assert_eq!(space.err(), Some(CardError::Key(' ')));
}

#[test]
fn seen_mark_has_a_no_script_form_and_needs_a_write() {
    let markup = Seen::new(action("seen", true))
        .unwrap()
        .render()
        .into_string();
    assert!(markup.starts_with("<shell-seen data-action="));
    assert!(markup.contains("<form method=\"post\" action=\"/v1/seen\">"));
    assert!(Seen::new(action("read", false)).is_err());
}

#[test]
fn frame_renders_notices_counterpart_and_feeds() {
    let feeds = [Feed {
        route: "/events",
        reference: "waiting",
        since: "p1",
        refresh: None,
    }];
    let mut frame = Frame::new(Product::Polylogue, "Timeline", "/q", "polylogue timeline");
    frame.feeds = &feeds;
    frame.notices = Some("/notices");
    frame.counterpart = Some(Counterpart::View {
        product: Product::Sinex,
        href: "https://sinex.test/day/2026-10-07",
        label: "Day 2026-10-07",
    });
    let page = frame
        .render(&(), &|_: &()| html! { p { "rows" } })
        .into_string();
    assert!(page.contains("<a href=\"/notices\" data-notices=\"\">Notices</a>"));
    assert!(page.contains("data-counterpart=\"\" href=\"https://sinex.test/day/2026-10-07\""));
    assert!(page.contains("<shell-feed data-route=\"/events\" data-reference=\"waiting\""));
    assert!(!page.contains("data-feed="));
    frame.counterpart = Some(Counterpart::Absent {
        product: Product::Sinex,
        reason: "no counterpart for a library item",
    });
    let page = frame.render(&(), &|_: &()| html! {}).into_string();
    assert!(page.contains("data-reason=\"no counterpart for a library item\""));
    // A product running alone renders nothing of the other.
    frame.counterpart = None;
    let page = frame.render(&(), &|_: &()| html! {}).into_string();
    assert!(!page.contains("data-counterpart"));
    assert!(!page.contains("Sinex"));
}

#[test]
fn same_key_same_act() {
    // Web and picker agree for every shared binding a picker offers.
    for binding in KEYS {
        if let Some(picker) = binding.picker {
            assert_eq!(picker, keys::picker_form(binding.key), "{}", binding.label);
        }
    }
    for (letter, terminal, expected) in [
        ("j", "Ctrl-j", KeyAction::Next),
        ("k", "Ctrl-k", KeyAction::Previous),
        ("y", "Ctrl-y", KeyAction::CopyMenu),
        ("x", "Ctrl-x", KeyAction::ToggleSelection),
        ("n", "Ctrl-n", KeyAction::NextMatch),
    ] {
        let binding = KEYS
            .iter()
            .find(|binding| binding.key == letter && !binding.ctrl && binding.prefix.is_none())
            .unwrap();
        assert_eq!(binding.action, expected);
        assert!(keys::picker_bindings().any(|(key, action)| key == terminal && action == expected));
    }
    // The shared table itself binds each key sequence once.
    for (index, binding) in KEYS.iter().enumerate() {
        assert!(
            !KEYS[index + 1..]
                .iter()
                .any(|other| other.web_key() == binding.web_key()),
            "{} is bound twice",
            binding.web_key()
        );
    }
    let go = |key| {
        KEYS.iter()
            .find(|binding| binding.prefix == Some("g") && binding.key == key)
            .map(|binding| binding.action)
    };
    assert_eq!(go("n"), Some(KeyAction::GoNotices));
    assert_eq!(go("o"), Some(KeyAction::GoCounterpart));
    assert!(
        KEYS.iter()
            .filter(|binding| binding.place == Place::Timeline)
            .all(|binding| binding.picker.is_none())
    );
}

const fn key(key: &'static str, act: &'static str) -> ProductKey {
    ProductKey {
        key,
        prefix: None,
        ctrl: false,
        act,
        picker: None,
    }
}
const fn go(key: &'static str, act: &'static str) -> ProductKey {
    ProductKey {
        key,
        prefix: Some("g"),
        ctrl: false,
        act,
        picker: None,
    }
}

#[test]
fn same_key_same_act_within_a_product() {
    let polylogue = [
        key("a", "add note"),
        key("c", "compare"),
        go("t", "timeline"),
        go("h", "home"),
    ];
    assert_eq!(keys::check_product(&polylogue), Ok(()));
    // The reserved go letters and every shared key are refused.
    let taken = [go("n", "next page"), key("j", "jump"), go("o", "overview")];
    let conflicts = keys::check_product(&taken).unwrap_err();
    assert_eq!(conflicts.len(), 3);
    assert!(
        conflicts
            .iter()
            .all(|conflict| matches!(conflict, KeyConflict::Shared { .. }))
    );
    // One key, one act; the picker takes the Ctrl form.
    let mixed = [key("a", "add note"), key("a", "approve")];
    assert!(matches!(
        keys::check_product(&mixed).unwrap_err()[0],
        KeyConflict::TwoActs { .. }
    ));
    let picker = [ProductKey {
        picker: Some("Alt-a"),
        ..key("a", "add note")
    }];
    assert!(matches!(
        keys::check_product(&picker).unwrap_err()[0],
        KeyConflict::Picker { .. }
    ));
    let picker = [ProductKey {
        picker: Some("Ctrl-Shift-s"),
        ..key("S", "save")
    }];
    assert_eq!(keys::check_product(&picker), Ok(()));
}

#[test]
fn same_key_same_act_between_products() {
    let polylogue = [
        key("a", "add note"),
        key("c", "compare"),
        go("s", "status"),
        go("t", "timeline"),
    ];
    let sinex = [
        key("a", "add note"),
        key("c", "compare"),
        go("s", "sources"),
        go("t", "timeline"),
    ];
    // Navigation letters name each product's own sections.
    assert_eq!(keys::check_between(&polylogue, &sinex), Ok(()));
    let renamed = [key("n", "add note"), key("c", "copy")];
    let conflicts = keys::check_between(&polylogue, &renamed).unwrap_err();
    assert!(
        conflicts.iter().any(
            |conflict| matches!(conflict, KeyConflict::TwoKeys { act, .. } if act == "add note")
        )
    );
    assert!(
        conflicts
            .iter()
            .any(|conflict| matches!(conflict, KeyConflict::TwoActs { key, .. } if key == "c"))
    );
}

#[test]
fn works_without_script() {
    // Every element wraps markup that works on its own: links, forms and tables.
    let card = Card::new(
        "Approve",
        html! { p { "body" } },
        vec![CardAction {
            key: 'a',
            action: action("approve", true),
        }],
    )
    .unwrap()
    .render()
    .into_string();
    let seen = Seen::new(action("seen", true))
        .unwrap()
        .render()
        .into_string();
    let copy = web_shell::CopyTarget {
        label: "reference".into(),
        forms: vec![web_shell::CopyForm {
            label: "Reference".into(),
            text: "session:a".into(),
        }],
    }
    .render(html! { code { "session:a" } })
    .into_string();
    for markup in [&card, &seen] {
        assert!(markup.contains("<form method=\"post\""), "{markup}");
    }
    assert!(copy.starts_with("<shell-copy data-copy="));
    assert!(copy.contains("<code>session:a</code>"));
    for markup in [&card, &seen, &copy] {
        assert!(!markup.contains("<script"));
        assert!(!markup.contains(" style="));
    }
}
