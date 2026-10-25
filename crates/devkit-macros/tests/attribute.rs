#[devkit_macros::test]
fn a_plain_test_runs() {
    assert_eq!(1 + 1, 2);
}

#[devkit_macros::test]
async fn an_async_test_runs() {
    tokio::task::yield_now().await;
}

#[devkit_macros::test(timeout = "5s")]
async fn an_async_test_with_a_timeout_returns_its_result() -> Result<(), String> {
    tokio::time::sleep(std::time::Duration::from_millis(1)).await;
    Ok(())
}

#[devkit_macros::test(timeout = "10ms")]
#[should_panic(expected = "test timed out after 10 ms")]
async fn a_slow_async_test_times_out() {
    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
}

#[devkit_macros::test(multi_thread)]
async fn a_multi_thread_test_runs_tasks_while_its_body_blocks() {
    // A blocked body would starve a current-thread runtime; here a worker
    // thread runs the spawned task and the channel receives its value.
    let (sender, receiver) = std::sync::mpsc::channel();
    tokio::spawn(async move { sender.send(7).unwrap() });
    let value = receiver
        .recv_timeout(std::time::Duration::from_secs(5))
        .unwrap();
    assert_eq!(value, 7);
}

#[devkit_macros::test(multi_thread, timeout = "5s")]
async fn a_multi_thread_test_takes_a_timeout() {
    let handle = tokio::runtime::Handle::current();
    assert_eq!(
        handle.runtime_flavor(),
        tokio::runtime::RuntimeFlavor::MultiThread
    );
}
