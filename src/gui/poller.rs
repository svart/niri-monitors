use std::sync::mpsc::{self, Receiver, RecvError, TryRecvError};
use std::thread;
use std::time::{Duration, Instant};

/// Runs a blocking fetch on a background thread, so the UI thread never waits on a socket.
pub struct Poller<T> {
    receiver: Option<Receiver<T>>,
    last_fetch: Option<Instant>,
}

impl<T> Default for Poller<T> {
    fn default() -> Self {
        Self {
            receiver: None,
            last_fetch: None,
        }
    }
}

impl<T: Send + 'static> Poller<T> {
    pub fn is_fetching(&self) -> bool {
        self.receiver.is_some()
    }

    /// Returns the result of a finished fetch, then starts `fetch` again if no fetch is
    /// running and `interval` has passed since the last one started. `Err` means the fetch
    /// thread ended without a result.
    pub fn poll(&mut self, interval: Duration, fetch: fn() -> T) -> Option<Result<T, RecvError>> {
        let finished = match self.receiver.as_ref().map(Receiver::try_recv) {
            Some(Ok(value)) => Some(Ok(value)),
            Some(Err(TryRecvError::Disconnected)) => Some(Err(RecvError)),
            Some(Err(TryRecvError::Empty)) | None => None,
        };
        if finished.is_some() {
            self.receiver = None;
        }

        if self.receiver.is_none()
            && self
                .last_fetch
                .is_none_or(|last_fetch| last_fetch.elapsed() >= interval)
        {
            let (sender, receiver) = mpsc::channel();
            self.receiver = Some(receiver);
            self.last_fetch = Some(Instant::now());
            thread::spawn(move || {
                let _ = sender.send(fetch());
            });
        }

        finished
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LONG_INTERVAL: Duration = Duration::from_secs(60);

    #[test]
    fn returns_the_fetched_value() {
        let mut poller = Poller::default();

        assert_eq!(wait_for_result(&mut poller, Duration::ZERO, || 7), Ok(7));
    }

    #[test]
    fn waits_for_the_interval_before_fetching_again() {
        let mut poller = Poller::default();
        wait_for_result(&mut poller, LONG_INTERVAL, || 7).expect("fetch should finish");

        assert!(poller.poll(LONG_INTERVAL, || 7).is_none());
        assert!(!poller.is_fetching());
    }

    #[test]
    fn reports_a_fetch_thread_that_ends_without_a_value() {
        let mut poller: Poller<i32> = Poller::default();

        let result = wait_for_result(&mut poller, LONG_INTERVAL, || panic!("fetch failed"));

        assert_eq!(result, Err(RecvError));
    }

    fn wait_for_result<T: Send + 'static>(
        poller: &mut Poller<T>,
        interval: Duration,
        fetch: fn() -> T,
    ) -> Result<T, RecvError> {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(result) = poller.poll(interval, fetch) {
                return result;
            }
            assert!(Instant::now() < deadline, "fetch did not finish");
            thread::sleep(Duration::from_millis(1));
        }
    }
}
