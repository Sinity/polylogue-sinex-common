use super::*;
use std::sync::atomic::{AtomicBool, Ordering};

#[tokio::test(start_paused = true)]
async fn phases_consume_one_parent_capped_deadline() {
    let start = Instant::now();
    let cancellation = CancellationToken::new();
    let context = AttemptContext::new(
        Duration::from_secs(30),
        Some(start + Duration::from_secs(10)),
        cancellation.clone(),
    );
    assert_eq!(context.deadline(), start + Duration::from_secs(10));
    let retry = context.clone();
    context
        .run(tokio::time::sleep(Duration::from_secs(6)))
        .await
        .unwrap();
    assert_eq!(retry.deadline(), context.deadline());
    assert_eq!(
        retry.run(tokio::time::sleep(Duration::from_secs(5))).await,
        Err(Interrupted::Deadline),
    );
    assert_eq!(Instant::now(), start + Duration::from_secs(10));
    cancellation.cancel();
    assert_eq!(retry.check(), Err(Interrupted::Cancelled));
}

#[tokio::test(start_paused = true)]
async fn expired_or_cancelled_context_does_not_poll_a_new_phase() {
    let cancellation = CancellationToken::new();
    let context = AttemptContext::new(Duration::from_secs(1), None, cancellation.clone());
    tokio::time::advance(Duration::from_secs(1)).await;
    let polled = AtomicBool::new(false);
    assert_eq!(
        context
            .run(async { polled.store(true, Ordering::SeqCst) })
            .await,
        Err(Interrupted::Deadline)
    );
    cancellation.cancel();
    assert_eq!(
        context
            .run(async { polled.store(true, Ordering::SeqCst) })
            .await,
        Err(Interrupted::Cancelled)
    );
    assert!(!polled.load(Ordering::SeqCst));
}

#[tokio::test]
async fn actual_owner_cancellation_interrupts_an_active_phase() {
    let cancellation = CancellationToken::new();
    let context = AttemptContext::new(Duration::from_secs(60), None, cancellation.clone());
    let (entered, wait) = tokio::sync::oneshot::channel();
    let phase = tokio::spawn(async move {
        context
            .run(async move {
                entered.send(()).unwrap();
                std::future::pending::<()>().await;
            })
            .await
    });
    wait.await.unwrap();
    cancellation.cancel();
    assert_eq!(phase.await.unwrap(), Err(Interrupted::Cancelled));
}

#[tokio::test]
async fn attempt_cancellation_does_not_cancel_owner_or_sibling() {
    let owner = CancellationToken::new();
    let first = AttemptContext::new(Duration::from_secs(60), None, owner.clone());
    let sibling = AttemptContext::new(Duration::from_secs(60), None, owner.clone());
    first.cancellation().cancel();
    assert_eq!(first.check(), Err(Interrupted::Cancelled));
    assert!(!owner.is_cancelled());
    assert_eq!(sibling.check(), Ok(()));
    owner.cancel();
    assert_eq!(sibling.check(), Err(Interrupted::Cancelled));
}
