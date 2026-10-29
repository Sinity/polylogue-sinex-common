use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use signals::DeliveryOutcome;
use signals::notifications::{
    Completion, DesktopEvent, Notice, NotificationReceipt, NotificationState, Route,
};
use signals::{Condition, Evaluator, HostPolicy};
use std::collections::BTreeSet;

#[derive(Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
struct Checkpoint {
    evaluator: Evaluator,
    host: HostPolicy,
    notifications: NotificationState,
    receipt: NotificationReceipt,
    condition: Condition,
}

#[test]
fn product_checkpoint_schema_contains_complete_typed_private_state() {
    let schema = schema_for!(Checkpoint).to_value();
    for (name, fields) in [
        (
            "Evaluator",
            vec![
                "now", "policy", "rules", "products", "inputs", "consumed", "pending",
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
            ],
        ),
        (
            "Pending",
            vec!["trigger", "due", "matched", "basis", "change", "value"],
        ),
        ("Snapshot", vec!["at", "order", "rows"]),
        (
            "HostPolicy",
            vec!["quiet_hours", "budget", "pending", "consumed", "sent"],
        ),
        ("Held", vec!["intent", "due"]),
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
                "dbus",
                "command",
                "notification_id",
                "notification_slot",
                "shown",
                "retried",
                "active",
                "server_owner",
                "handoff",
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
    assert_eq!(
        schema["$defs"]["Condition"]["enum"],
        json!(["matching", "ended", "incomplete"])
    );
}

#[test]
fn typed_checkpoint_and_receipt_keep_existing_wire_values() {
    let evaluator: Evaluator = serde_json::from_value(json!({
        "now": 42, "policy":{"private":[]}, "rules":{}, "products":[],
        "inputs":[], "consumed":[], "pending":{}
    }))
    .unwrap();
    let notice = Notice {
        delivery_id: "delivery".into(),
        product: "product".into(),
        icon: "utilities-terminal".into(),
        rule: "rule".into(),
        occurrence: "occurrence".into(),
        summary: "Synthetic signal".into(),
        body: "A current synthetic fact".into(),
        reference: "record:example".into(),
        command_hint: "read record:example".into(),
        interrupt: true,
        current: true,
        data_loss: false,
        budget_passive_count: None,
    };
    let mut notifications = NotificationState::default();
    notifications.begin(notice, Route::Dbus);
    notifications.completed(
        "delivery",
        Route::Dbus,
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
            route: Route::Dbus,
            outcome: DeliveryOutcome::Delivered,
            notification_id: Some(7),
            event: Some(DesktopEvent::Acknowledged),
            via: Some("desktop".into()),
        },
        condition: Condition::Matching,
    };
    let wire = serde_json::to_value(&checkpoint).unwrap();
    assert_eq!(wire["condition"], "matching");
    assert_eq!(wire["receipt"]["route"], "dbus");
    assert_eq!(wire["receipt"]["event"], json!({"event":"acknowledged"}));
    assert_eq!(
        wire["notifications"]["attempts"]["delivery"]["notice"]["reference"],
        "record:example"
    );
    assert_eq!(
        serde_json::from_value::<Checkpoint>(wire).unwrap(),
        checkpoint
    );
    assert!(serde_json::from_value::<Condition>(Value::String("other".into())).is_err());
}

#[test]
fn imported_attempt_is_immutable_across_receiver_progress_and_restart() {
    let mut source = NotificationState::default();
    let mut notice:Notice=serde_json::from_value(json!({"delivery_id":"a","product":"p","icon":"i","rule":"r","occurrence":"o","summary":"s","body":"b","reference":"record:a","command_hint":"","interrupt":true,"current":true,"data_loss":false,"budget_passive_count":null})).unwrap();
    source.begin(notice.clone(), Route::Both);
    source.completed(
        "a",
        Route::Dbus,
        Completion {
            outcome: DeliveryOutcome::NotSent("no_notification_server".into()),
            notification_id: None,
            server_owner: None,
            slot: None,
        },
    );
    let original = source.attempt("a").unwrap().clone();
    let mut receiver = NotificationState::default();
    assert_eq!(
        receiver.import_attempt(original.clone()),
        Ok(signals::notifications::ImportStatus::Imported)
    );
    receiver.prepare_retry("a");
    receiver.completed(
        "a",
        Route::Dbus,
        Completion {
            outcome: DeliveryOutcome::Delivered,
            notification_id: Some(7),
            server_owner: Some(":1.2".into()),
            slot: None,
        },
    );
    let mut restored: NotificationState =
        serde_json::from_slice(&serde_json::to_vec(&receiver).unwrap()).unwrap();
    assert_eq!(
        restored.import_attempt(original),
        Ok(signals::notifications::ImportStatus::Known)
    );
    assert!(restored.attempt("a").unwrap().retried);
    assert_eq!(
        restored.attempt("a").unwrap().dbus,
        Some(DeliveryOutcome::Delivered)
    );
    assert_eq!(
        restored.attempt("a").unwrap().command,
        Some(DeliveryOutcome::Unknown)
    );
    notice.body = "different".into();
    let mut conflict = source.attempt("a").unwrap().clone();
    conflict.notice = notice;
    assert_eq!(
        restored.import_attempt(conflict),
        Err(signals::notifications::ImportError::Conflict)
    );
    assert!(restored.server_appeared().is_empty());
}
