use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use signals::DeliveryOutcome;
use signals::notifications::{
    Completion, DesktopEvent, Notice, NotificationReceipt, NotificationState, Outlet, Outlets,
};
use signals::{Evaluator, HostPolicy};
use std::collections::BTreeSet;

#[derive(Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
struct Checkpoint {
    evaluator: Evaluator,
    host: HostPolicy,
    notifications: NotificationState,
    receipt: NotificationReceipt,
}

#[test]
fn product_checkpoint_schema_contains_complete_typed_private_state() {
    let schema = schema_for!(Checkpoint).to_value();
    for (name, fields) in [
        (
            "Evaluator",
            vec![
                "now",
                "rules",
                "products",
                "inputs",
                "consumed",
                "pending",
                "open",
                "retractions",
                "reports",
            ],
        ),
        (
            "RuleState",
            vec![
                "active",
                "started",
                "revision_baseline",
                "rows",
                "order",
                "incomplete",
                "baseline",
                "latest",
                "reported",
                "change_due",
                "threshold",
                "last_value",
                "observed_at",
                "next_at",
                "rebuilt",
            ],
        ),
        (
            "Pending",
            vec!["trigger", "due", "matched", "basis", "change", "value"],
        ),
        ("Snapshot", vec!["at", "order", "rows"]),
        ("Open", vec!["trigger", "basis", "change"]),
        (
            "HostPolicy",
            vec![
                "settings",
                "budget",
                "now",
                "attention",
                "inputs",
                "pending",
                "consumed",
                "sent",
                "rules",
                "progress",
                "open",
                "caused",
                "away",
                "budget_passive",
                "keys",
                "news",
                "rearmed_late",
            ],
        ),
        (
            "Held",
            vec!["fired", "due", "holds", "deferred_since", "release"],
        ),
        (
            "NotificationState",
            vec![
                "attempts",
                "slots",
                "first_occurrence",
                "server_owner",
                "shown_delivery",
            ],
        ),
        (
            "Attempt",
            vec![
                "notice",
                "desktop",
                "command",
                "notification_id",
                "notification_slot",
                "shown",
                "retried",
                "active",
                "server_owner",
            ],
        ),
    ] {
        let definition = &schema["$defs"][name];
        assert_eq!(
            definition["type"], "object",
            "{name} must be a typed checkpoint"
        );
        let actual: BTreeSet<_> = definition["properties"]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(actual, fields.into_iter().collect(), "{name} wire fields");
    }
    assert_eq!(
        schema["$defs"]["NotificationState"]["properties"]["attempts"]["additionalProperties"]["$ref"],
        "#/$defs/Attempt"
    );
}

#[test]
fn typed_checkpoint_and_receipt_keep_existing_wire_values() {
    let evaluator: Evaluator = serde_json::from_value(json!({
        "now": 42, "rules":{}, "products":[],
        "inputs":[], "consumed":[], "pending":{}, "open":{}, "reports":[]
    }))
    .unwrap();
    let notice = Notice {
        delivery_id: "delivery".into(),
        product: "product".into(),
        icon: "utilities-terminal".into(),
        desktop_entry: None,
        rule: "rule".into(),
        occurrence: "occurrence".into(),
        summary: "Synthetic signal".into(),
        body: "A current synthetic fact".into(),
        lines: vec![],
        reference: "record:example".into(),
        command_hint: "read record:example".into(),
        current: true,
        transient: false,
        critical: false,
        outlets: Outlets {
            desktop: true,
            command: false,
        },
        actions: vec![],
        showing: 0,
    };
    let mut notifications = NotificationState::default();
    notifications.begin(notice);
    notifications.completed(
        "delivery",
        Outlet::Desktop,
        Completion {
            outcome: DeliveryOutcome::Delivered,
            notification_id: Some(7),
            server_owner: None,
            slot: None,
        },
    );
    let checkpoint = Checkpoint {
        evaluator,
        host: HostPolicy::default(),
        notifications,
        receipt: NotificationReceipt {
            delivery_id: "delivery".into(),
            outlet: Outlet::Desktop,
            outcome: DeliveryOutcome::Delivered,
            notification_id: Some(7),
            event: Some(DesktopEvent::Acknowledged),
            via: Some("desktop".into()),
        },
    };
    let wire = serde_json::to_value(&checkpoint).unwrap();
    assert_eq!(wire["receipt"]["outlet"], "desktop");
    assert_eq!(wire["receipt"]["event"], json!({"event":"acknowledged"}));
    assert_eq!(
        wire["notifications"]["attempts"]["delivery"]["notice"]["reference"],
        "record:example"
    );
    assert_eq!(
        serde_json::from_value::<Checkpoint>(wire).unwrap(),
        checkpoint
    );
    assert!(serde_json::from_value::<Outlet>(Value::String("dbus".into())).is_err());
}
