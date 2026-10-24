//! `#[devkit::test]` creates each parameter through `devkit::Fixture`.
use devkit::Fixture;
use std::sync::atomic::{AtomicUsize, Ordering};

static CREATED: AtomicUsize = AtomicUsize::new(0);

/// Numbers its values in creation order.
struct Counted(usize);

impl Fixture for Counted {
    fn create() -> Result<Self, String> {
        Ok(Counted(CREATED.fetch_add(1, Ordering::SeqCst) + 1))
    }
}

struct Unavailable;

impl Fixture for Unavailable {
    fn create() -> Result<Self, String> {
        Err("no server".into())
    }
}

#[devkit::test]
fn fixtures_are_created_in_parameter_order(first: Counted, second: Counted) {
    assert!(first.0 < second.0);
}

#[devkit::test]
fn a_test_with_fixtures_can_return_a_result(mut counted: Counted) -> Result<(), String> {
    counted.0 += 1;
    if counted.0 > 1 {
        Ok(())
    } else {
        Err("counted from zero".into())
    }
}

#[devkit::test(timeout = "5s")]
async fn an_async_test_gets_its_fixtures(counted: Counted) {
    tokio::task::yield_now().await;
    assert!(counted.0 >= 1);
}

#[devkit::test]
#[should_panic(expected = "fixture Unavailable: no server")]
fn a_fixture_that_cannot_be_created_fails_the_test(_unavailable: Unavailable) {}
