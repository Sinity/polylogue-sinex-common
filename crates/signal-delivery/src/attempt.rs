//! Process-local lifetime of one owner-dispatched attempt.
use std::{future::Future, time::Duration};
use tokio::time::Instant;
pub use tokio_util::sync::CancellationToken;

/// One absolute deadline and the actual owner's cancellation, shared by every
/// phase and policy retry. This context carries no delivery authority and is
/// never serialized into a signal checkpoint.
#[derive(Clone, Debug)]
pub struct AttemptContext {
    deadline: Instant,
    cancellation: CancellationToken,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Interrupted {
    Deadline,
    Cancelled,
}

impl Interrupted {
    pub fn reason(self) -> &'static str {
        match self {
            Self::Deadline => "attempt_deadline",
            Self::Cancelled => "attempt_cancelled",
        }
    }
}

impl std::fmt::Display for Interrupted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.reason())
    }
}
impl std::error::Error for Interrupted {}

impl AttemptContext {
    /// Called once at dispatch, before unknown persistence, policy or platform
    /// preparation. A child can shorten an existing parent deadline only.
    pub fn new(
        budget: Duration,
        parent_deadline: Option<Instant>,
        cancellation: CancellationToken,
    ) -> Self {
        let deadline = Instant::now() + budget;
        Self {
            deadline: parent_deadline.map_or(deadline, |parent| parent.min(deadline)),
            cancellation: cancellation.child_token(),
        }
    }

    pub fn deadline(&self) -> Instant {
        self.deadline
    }

    pub fn cancellation(&self) -> &CancellationToken {
        &self.cancellation
    }

    pub fn check(&self) -> Result<(), Interrupted> {
        if self.cancellation.is_cancelled() {
            Err(Interrupted::Cancelled)
        } else if Instant::now() >= self.deadline {
            Err(Interrupted::Deadline)
        } else {
            Ok(())
        }
    }

    /// Bounds one cancellation-safe phase without renewing its lifetime. On
    /// interruption the future is dropped. Transaction owners must catch this
    /// result inside their scope and acknowledge rollback before returning an
    /// authorization; do not wrap an entire live transaction in this method.
    pub async fn run<F: Future>(&self, phase: F) -> Result<F::Output, Interrupted> {
        self.check()?;
        tokio::select! {
            biased;
            _ = self.cancellation.cancelled() => Err(Interrupted::Cancelled),
            _ = tokio::time::sleep_until(self.deadline) => Err(Interrupted::Deadline),
            value = phase => Ok(value),
        }
    }
}

#[cfg(test)]
mod tests;
