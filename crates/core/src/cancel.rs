use std::error::Error;
use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// A cheap-to-clone cooperative cancellation signal.
///
/// Worker code should check the token at bounded intervals and before publishing
/// results. Dropping a token does not cancel its siblings; call [`Self::cancel`].
#[derive(Clone, Debug, Default)]
pub struct CancelToken {
    cancelled: Arc<AtomicBool>,
}

impl CancelToken {
    /// Creates an uncancelled token.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Requests cancellation for this token and all of its clones.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    /// Returns whether cancellation has been requested.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }

    /// Converts the cancellation state into a result convenient for worker loops.
    ///
    /// # Errors
    ///
    /// Returns [`Cancelled`] after cancellation has been requested.
    pub fn check(&self) -> Result<(), Cancelled> {
        if self.is_cancelled() {
            Err(Cancelled)
        } else {
            Ok(())
        }
    }
}

/// Error returned when cooperative work observes a cancellation request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Cancelled;

impl fmt::Display for Cancelled {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("operation cancelled")
    }
}

impl Error for Cancelled {}

#[cfg(test)]
mod tests {
    use super::{CancelToken, Cancelled};

    #[test]
    fn cancellation_is_shared_by_clones() {
        let token = CancelToken::new();
        let worker_token = token.clone();

        assert_eq!(worker_token.check(), Ok(()));
        token.cancel();

        assert!(worker_token.is_cancelled());
        assert_eq!(worker_token.check(), Err(Cancelled));
    }
}
