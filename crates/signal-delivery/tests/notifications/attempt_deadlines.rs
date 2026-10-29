use super::*;
use signal_delivery::{Interrupted, PolicyFailure};
use std::sync::atomic::{AtomicUsize, Ordering};

fn command(path: &std::path::Path) -> NotificationConfig {
    NotificationConfig {
        route: Route::Command,
        command: Some(RouteCommand {
            program: "sh".into(),
            arguments: vec![
                "-c".into(),
                "printf effect > \"$1\"".into(),
                "route".into(),
                path.to_string_lossy().into_owned(),
            ],
        }),
        deadline: Duration::from_secs(10),
    }
}

#[tokio::test]
async fn cancellation_during_dbus_preparation_is_not_sent_and_skips_policy() {
    let bus = PrivateBus::start();
    let records = Records::default();
    records.stall_capabilities.store(true, Ordering::SeqCst);
    let _server = bus
        .server(FakeServer {
            records: records.clone(),
            actions: true,
            stall: false,
        })
        .await;
    let mut deliverer = Deliverer::new(bus.connect().await, config(), NotificationState::default());
    let owner = CancellationToken::new();
    let context = AttemptContext::new(Duration::from_secs(60), None, owner.clone());
    let calls = Arc::new(AtomicUsize::new(0));
    let check: signal_delivery::PolicyCheck = {
        let calls = calls.clone();
        Arc::new(move |request| {
            calls.fetch_add(1, Ordering::SeqCst);
            permit(request)
        })
    };
    deliverer.begin(notice("prepare", "one"));
    let sending =
        tokio::spawn(async move { deliverer.send("prepare", check.as_ref(), &context).await });
    records.capabilities_entered.notified().await;
    owner.cancel();
    let receipts = sending.await.unwrap();
    assert_eq!(
        receipts[0].outcome,
        DeliveryOutcome::NotSent("attempt_cancelled".into())
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert!(records.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn both_routes_share_deadline_and_owner_cancellation_without_a_fresh_budget() {
    let bus = PrivateBus::start();
    let records = Records::default();
    let _server = bus
        .server(FakeServer {
            records: records.clone(),
            actions: true,
            stall: false,
        })
        .await;
    let directory = tempfile::tempdir().unwrap();
    let output = directory.path().join("effect");
    let mut configuration = command(&output);
    configuration.route = Route::Both;
    let mut deliverer = Deliverer::new(
        bus.connect().await,
        configuration,
        NotificationState::default(),
    );
    let token = CancellationToken::new();
    let context = AttemptContext::new(Duration::from_secs(60), None, token.clone());
    let deadline = context.deadline();
    let calls = Arc::new(AtomicUsize::new(0));
    let check: signal_delivery::PolicyCheck = {
        let calls = calls.clone();
        Arc::new(move |request| {
            assert_eq!(request.attempt.deadline(), deadline);
            let ordinal = calls.fetch_add(1, Ordering::SeqCst);
            if ordinal == 1 {
                token.cancel();
            }
            permit(request)
        })
    };
    deliverer.begin(notice("both", "one"));
    let receipts = deliverer.send("both", check.as_ref(), &context).await;
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert_eq!(receipts[0].route, Route::Dbus);
    assert_eq!(receipts[0].outcome, DeliveryOutcome::Delivered);
    assert_eq!(receipts[1].route, Route::Command);
    assert_eq!(
        receipts[1].outcome,
        DeliveryOutcome::NotSent("attempt_cancelled".into())
    );
    assert!(!output.exists());
    assert_eq!(records.calls.lock().unwrap().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn policy_retries_consume_original_budget_and_cannot_spawn_after_expiry() {
    let directory = tempfile::tempdir().unwrap();
    let output = directory.path().join("effect");
    let mut deliverer = Deliverer::new(None, command(&output), NotificationState::default());
    let context = AttemptContext::new(Duration::from_secs(10), None, CancellationToken::new());
    let original = context.deadline();
    let checks = Arc::new(AtomicUsize::new(0));
    let check: signal_delivery::PolicyCheck = {
        let checks = checks.clone();
        Arc::new(move |request| {
            let checks = checks.clone();
            Box::pin(async move {
                assert_eq!(request.attempt.deadline(), original);
                request
                    .attempt
                    .run(tokio::time::sleep(Duration::from_secs(6)))
                    .await
                    .unwrap();
                checks.fetch_add(1, Ordering::SeqCst);
                let retry = request.attempt.clone();
                assert_eq!(retry.deadline(), original);
                assert_eq!(
                    retry.run(tokio::time::sleep(Duration::from_secs(5))).await,
                    Err(Interrupted::Deadline)
                );
                checks.fetch_add(1, Ordering::SeqCst);
                // Even an erroneously late positive answer cannot authorize I/O.
                authorize(&request.notices, SendDecision::Permit)
            })
        })
    };
    deliverer.begin(notice("deadline", "one"));
    let receipts = deliverer.send("deadline", check.as_ref(), &context).await;
    assert_eq!(checks.load(Ordering::SeqCst), 2);
    assert_eq!(
        receipts[0].outcome,
        DeliveryOutcome::NotSent("attempt_deadline".into())
    );
    assert!(!output.exists());
}

#[tokio::test]
async fn cancellation_waits_for_owner_cleanup_and_prevents_platform_io() {
    let directory = tempfile::tempdir().unwrap();
    let output = directory.path().join("effect");
    let mut deliverer = Deliverer::new(None, command(&output), NotificationState::default());
    let token = CancellationToken::new();
    let context = AttemptContext::new(Duration::from_secs(60), None, token.clone());
    let entered = Arc::new(tokio::sync::Notify::new());
    let interrupted = Arc::new(tokio::sync::Notify::new());
    let rollback = Arc::new(tokio::sync::Notify::new());
    let check: signal_delivery::PolicyCheck = {
        let entered = entered.clone();
        let interrupted = interrupted.clone();
        let rollback = rollback.clone();
        Arc::new(move |request| {
            let entered = entered.clone();
            let interrupted = interrupted.clone();
            let rollback = rollback.clone();
            Box::pin(async move {
                entered.notify_one();
                assert_eq!(
                    request.attempt.run(std::future::pending::<()>()).await,
                    Err(Interrupted::Cancelled)
                );
                interrupted.notify_one();
                // Represents an explicit asynchronous rollback acknowledgement.
                rollback.notified().await;
                authorize(&request.notices, SendDecision::Permit)
            })
        })
    };
    deliverer.begin(notice("cancel", "one"));
    let sending =
        tokio::spawn(async move { deliverer.send("cancel", check.as_ref(), &context).await });
    entered.notified().await;
    token.cancel();
    interrupted.notified().await;
    assert!(!sending.is_finished());
    assert!(!output.exists());
    rollback.notify_one();
    let receipts = sending.await.unwrap();
    assert_eq!(
        receipts[0].outcome,
        DeliveryOutcome::NotSent("attempt_cancelled".into())
    );
    assert!(!output.exists());
}

#[tokio::test(start_paused = true)]
async fn cleanup_can_outlive_work_deadline_without_granting_more_execution_time() {
    let directory = tempfile::tempdir().unwrap();
    let output = directory.path().join("effect");
    let mut deliverer = Deliverer::new(None, command(&output), NotificationState::default());
    let context = AttemptContext::new(Duration::from_secs(1), None, CancellationToken::new());
    let interrupted = Arc::new(tokio::sync::Notify::new());
    let rollback = Arc::new(tokio::sync::Notify::new());
    let check: signal_delivery::PolicyCheck = {
        let interrupted = interrupted.clone();
        let rollback = rollback.clone();
        Arc::new(move |request| {
            let interrupted = interrupted.clone();
            let rollback = rollback.clone();
            Box::pin(async move {
                let failure = request
                    .attempt
                    .run(std::future::pending::<()>())
                    .await
                    .unwrap_err();
                assert_eq!(failure, Interrupted::Deadline);
                interrupted.notify_one();
                rollback.notified().await;
                Err(PolicyFailure::Interrupted(failure))
            })
        })
    };
    deliverer.begin(notice("cleanup", "one"));
    let sending =
        tokio::spawn(async move { deliverer.send("cleanup", check.as_ref(), &context).await });
    interrupted.notified().await;
    tokio::time::advance(Duration::from_secs(50)).await;
    assert!(!sending.is_finished());
    assert!(!output.exists());
    rollback.notify_one();
    let receipts = sending.await.unwrap();
    assert_eq!(
        receipts[0].outcome,
        DeliveryOutcome::NotSent("attempt_deadline".into())
    );
    assert!(!output.exists());
}

#[tokio::test]
async fn cancelled_notify_remains_unknown_and_never_retries_after_restore() {
    let bus = PrivateBus::start();
    let records = Records::default();
    let _server = bus
        .server(FakeServer {
            records: records.clone(),
            actions: true,
            stall: true,
        })
        .await;
    let connection = bus.connect().await;
    let mut deliverer = Deliverer::new(connection.clone(), config(), NotificationState::default());
    let token = CancellationToken::new();
    let context = AttemptContext::new(Duration::from_secs(60), None, token.clone());
    deliverer.begin(notice("possible", "one"));
    let sending = tokio::spawn(async move {
        let receipts = deliverer.send("possible", &permit, &context).await;
        (deliverer.state, receipts)
    });
    records.entered.notified().await;
    token.cancel();
    let (state, receipts) = sending.await.unwrap();
    assert_eq!(receipts[0].outcome, DeliveryOutcome::Unknown);
    let mut restored = Deliverer::new(connection, config(), state);
    assert!(restored.begin(notice("possible", "one")).is_empty());
    assert!(restored.server_appeared().is_empty());
    assert!(
        restored
            .send("possible", &permit, &attempt())
            .await
            .is_empty()
    );
    assert_eq!(records.calls.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn one_dispatch_keeps_issued_member_unknown_and_unissued_member_not_sent() {
    let bus = PrivateBus::start();
    let records = Records::default();
    let _server = bus
        .server(FakeServer {
            records: records.clone(),
            actions: true,
            stall: true,
        })
        .await;
    let mut deliverer = Deliverer::new(bus.connect().await, config(), NotificationState::default());
    let token = CancellationToken::new();
    let context = AttemptContext::new(Duration::from_secs(60), None, token.clone());
    deliverer.begin(notice("first", "one"));
    deliverer.begin(notice("second", "two"));
    // The owner persists these two unknown routes together before dispatch.
    let _persisted = serde_json::to_vec(&deliverer.state).unwrap();
    let sending = tokio::spawn(async move {
        let mut receipts = deliverer.send("first", &permit, &context).await;
        receipts.extend(deliverer.send("second", &permit, &context).await);
        (deliverer.state, receipts)
    });
    records.entered.notified().await;
    token.cancel();
    let (state, receipts) = sending.await.unwrap();
    assert_eq!(receipts.len(), 2);
    assert_eq!(receipts[0].delivery_id, "first");
    assert_eq!(receipts[0].outcome, DeliveryOutcome::Unknown);
    assert_eq!(receipts[1].delivery_id, "second");
    assert_eq!(
        receipts[1].outcome,
        DeliveryOutcome::NotSent("attempt_cancelled".into())
    );
    assert_eq!(
        state.attempt("first").unwrap().dbus,
        Some(DeliveryOutcome::Unknown)
    );
    assert_eq!(records.calls.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn grouped_refreshes_keep_the_clear_dispatch_lifetime_without_losing_ready_ids() {
    let bus = PrivateBus::start();
    let records = Records::default();
    let _server = bus
        .server(FakeServer {
            records: records.clone(),
            actions: true,
            stall: false,
        })
        .await;
    let mut deliverer = Deliverer::new(bus.connect().await, config(), NotificationState::default());
    for (id, occurrence) in [("a", "one"), ("b", "two"), ("c", "three")] {
        send(&mut deliverer, notice(id, occurrence)).await;
    }
    let owner = CancellationToken::new();
    // A new clear dispatch does not reuse any original Notify deadline.
    let context = AttemptContext::new(Duration::from_secs(60), None, owner.clone());
    deliverer.cancel("a", &permit, &context).await.unwrap();
    let ready = deliverer.ready_ids();
    assert!(!ready.is_empty());
    // Unknown refresh custody precedes effects and shares the clear lifetime.
    let _persisted = serde_json::to_vec(&deliverer.state).unwrap();
    let before = records.calls.lock().unwrap().len();
    owner.cancel();
    let mut receipts = Vec::new();
    for id in &ready {
        receipts.extend(deliverer.send(id, &permit, &context).await);
    }
    assert_eq!(receipts.len(), ready.len());
    for id in ready {
        assert!(receipts.iter().any(|receipt| receipt.delivery_id == id
            && receipt.outcome == DeliveryOutcome::NotSent("attempt_cancelled".into())));
    }
    assert!(deliverer.ready_ids().is_empty());
    assert_eq!(records.calls.lock().unwrap().len(), before);
}

#[tokio::test(start_paused = true)]
async fn expired_parent_skips_policy_and_command_and_unknown_ack_consumes_budget() {
    let directory = tempfile::tempdir().unwrap();
    let output = directory.path().join("effect");
    let calls = Arc::new(AtomicUsize::new(0));
    let check: signal_delivery::PolicyCheck = {
        let calls = calls.clone();
        Arc::new(move |request| {
            calls.fetch_add(1, Ordering::SeqCst);
            permit(request)
        })
    };
    let (requests, received) = mpsc::unbounded_channel();
    let (updates, mut updated) = mpsc::unbounded_channel();
    let parent = tokio::time::Instant::now() + Duration::from_secs(1);
    let worker = tokio::spawn(run(
        Deliverer::new(None, command(&output), NotificationState::default()),
        received,
        updates,
        check,
        Arc::new(|| 0),
        CancellationToken::new(),
        Some(parent),
    ));
    requests
        .send(NotificationRequest::Deliver(Box::new(notice(
            "waiting", "one",
        ))))
        .unwrap();
    let initial = raw_update(&mut updated).await;
    assert_eq!(
        initial.state.attempt("waiting").unwrap().command,
        Some(DeliveryOutcome::Unknown)
    );
    tokio::time::advance(Duration::from_secs(1)).await;
    assert!(
        !worker.is_finished(),
        "submitted custody ACK is still observed"
    );
    initial.persisted.send(()).unwrap();
    let terminal = raw_update(&mut updated).await;
    assert_eq!(
        terminal.receipts[0].outcome,
        DeliveryOutcome::NotSent("attempt_deadline".into())
    );
    terminal.persisted.send(()).unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert!(!output.exists());
    drop(requests);
    worker.await.unwrap().unwrap();
}

#[tokio::test]
async fn unavailable_or_incomplete_policy_is_not_an_authoritative_cancellation() {
    for incomplete in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let output = directory.path().join("effect");
        let configuration = command(&output);
        let mut deliverer =
            Deliverer::new(None, configuration.clone(), NotificationState::default());
        let context = AttemptContext::new(configuration.deadline, None, CancellationToken::new());
        let check: signal_delivery::PolicyCheck = Arc::new(move |_| {
            Box::pin(async move {
                if incomplete {
                    Ok(Authorization {
                        decisions: Default::default(),
                    })
                } else {
                    Err(PolicyFailure::Unavailable)
                }
            })
        });
        deliverer.begin(notice("unavailable", "one"));
        let receipts = deliverer
            .send("unavailable", check.as_ref(), &context)
            .await;
        assert_eq!(
            receipts[0].outcome,
            DeliveryOutcome::NotSent(
                if incomplete {
                    "policy_incomplete"
                } else {
                    "policy_unavailable"
                }
                .into()
            )
        );
        assert!(deliverer.state.attempt("unavailable").unwrap().active);
        assert!(!output.exists());
        let mut restored = Deliverer::new(None, configuration, deliverer.state);
        assert!(restored.begin(notice("unavailable", "one")).is_empty());
        assert!(
            restored
                .send("unavailable", &permit, &attempt())
                .await
                .is_empty()
        );
        assert!(!output.exists());
    }
}

#[tokio::test]
async fn failed_clear_preserves_original_checkpoint_instead_of_cancelling_it() {
    let directory = tempfile::tempdir().unwrap();
    let mut deliverer = Deliverer::new(
        None,
        command(&directory.path().join("effect")),
        NotificationState::default(),
    );
    deliverer.begin(notice("clear", "one"));
    let before = serde_json::to_vec(&deliverer.state).unwrap();
    let check: signal_delivery::PolicyCheck =
        Arc::new(|_| Box::pin(async { Err(PolicyFailure::Unavailable) }));
    assert_eq!(
        deliverer.cancel("clear", check.as_ref(), &attempt()).await,
        Err(PolicyFailure::Unavailable)
    );
    assert_eq!(serde_json::to_vec(&deliverer.state).unwrap(), before);
    let incomplete: signal_delivery::PolicyCheck = Arc::new(|_| {
        Box::pin(async {
            Ok(Authorization {
                decisions: Default::default(),
            })
        })
    });
    assert_eq!(
        deliverer
            .cancel("clear", incomplete.as_ref(), &attempt())
            .await,
        Err(PolicyFailure::Unavailable)
    );
    assert_eq!(serde_json::to_vec(&deliverer.state).unwrap(), before);
}

#[tokio::test]
async fn omitted_group_contributor_does_not_cancel_members_or_send_a_partial_summary() {
    let bus = PrivateBus::start();
    let records = Records::default();
    let _server = bus
        .server(FakeServer {
            records: records.clone(),
            actions: true,
            stall: false,
        })
        .await;
    let mut deliverer = Deliverer::new(bus.connect().await, config(), NotificationState::default());
    send(&mut deliverer, notice("a", "one")).await;
    send(&mut deliverer, notice("b", "two")).await;
    let before = records.calls.lock().unwrap().len();
    let owner = deliverer.state.server_owner().unwrap().to_owned();
    let check: signal_delivery::PolicyCheck = Arc::new(|request| {
        Box::pin(async move {
            assert_eq!(request.notices.len(), 3);
            Ok(Authorization {
                decisions: [(request.delivery_id, SendDecision::Permit)].into(),
            })
        })
    });
    let context = attempt();
    deliverer.begin(notice("c", "three"));
    let receipts = deliverer.send("c", check.as_ref(), &context).await;
    assert_eq!(receipts.len(), 1);
    assert_eq!(
        receipts[0].outcome,
        DeliveryOutcome::NotSent("policy_incomplete".into())
    );
    assert_eq!(
        deliverer
            .state
            .attempt("c")
            .unwrap()
            .server_owner
            .as_deref(),
        Some(owner.as_str())
    );
    assert!(deliverer.state.attempts().values().all(|a| a.active));
    assert_eq!(records.calls.lock().unwrap().len(), before);
}

#[tokio::test(start_paused = true)]
async fn definitive_policy_denial_remains_cancelled_after_work_deadline() {
    let directory = tempfile::tempdir().unwrap();
    let output = directory.path().join("effect");
    let mut deliverer = Deliverer::new(None, command(&output), NotificationState::default());
    let context = AttemptContext::new(Duration::from_secs(1), None, CancellationToken::new());
    let check: signal_delivery::PolicyCheck = Arc::new(|request| {
        Box::pin(async move {
            let _ = request
                .attempt
                .run(tokio::time::sleep(Duration::from_secs(2)))
                .await;
            // The owner has definitive denial evidence even though cleanup used
            // the remaining work time. Denial never starts a platform operation.
            authorize(&request.notices, SendDecision::Cancel)
        })
    });
    deliverer.begin(notice("denied", "one"));
    let receipts = deliverer.send("denied", check.as_ref(), &context).await;
    assert_eq!(receipts[0].outcome, DeliveryOutcome::Cancelled);
    assert!(!deliverer.state.attempt("denied").unwrap().active);
    assert!(!output.exists());
}

#[tokio::test]
async fn expired_definitive_denial_cancels_without_close_or_notify_io() {
    let bus = PrivateBus::start();
    let records = Records::default();
    let _server = bus
        .server(FakeServer {
            records: records.clone(),
            actions: true,
            stall: false,
        })
        .await;
    let mut deliverer = Deliverer::new(bus.connect().await, config(), NotificationState::default());
    send(&mut deliverer, notice("shown", "one")).await;
    let shown_id = deliverer.state.attempt("shown").unwrap().notification_id;
    assert!(shown_id.is_some());
    let before = records.calls.lock().unwrap().len();
    let context = attempt();
    let check: signal_delivery::PolicyCheck = Arc::new(|request| {
        Box::pin(async move {
            // Preflight already finished on the private bus. Advance only the
            // controlled work clock while returning definite owner denial.
            tokio::time::pause();
            tokio::time::advance(Duration::from_secs(3)).await;
            assert_eq!(request.attempt.check(), Err(Interrupted::Deadline));
            authorize(&request.notices, SendDecision::Cancel)
        })
    });
    deliverer.begin(notice("new", "two"));
    let receipts = deliverer.send("new", check.as_ref(), &context).await;
    assert!(receipts.iter().any(
        |receipt| receipt.delivery_id == "new" && receipt.outcome == DeliveryOutcome::Cancelled
    ));
    assert!(!deliverer.state.attempt("shown").unwrap().active);
    assert!(!deliverer.state.attempt("new").unwrap().active);
    assert_eq!(records.calls.lock().unwrap().len(), before);
    assert!(records.closed.lock().unwrap().is_empty());
}
