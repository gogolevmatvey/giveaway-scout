use std::sync::atomic::{AtomicBool, Ordering};

/// Serializes short result commits and native delivery with data cleanup.
/// Network work holds no guard; cleanup cancels it using generation tokens.
#[derive(Default)]
pub struct DataOperationGate {
    occupied: AtomicBool,
    released: tokio::sync::Notify,
}

impl DataOperationGate {
    pub fn try_acquire(&self) -> Option<DataOperationGuard<'_>> {
        self.occupied
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .ok()
            .map(|_| DataOperationGuard(self))
    }

    pub(crate) fn is_available(&self) -> bool {
        !self.occupied.load(Ordering::Acquire)
    }

    pub(crate) async fn wait_available(&self) {
        loop {
            let released = self.released.notified();
            tokio::pin!(released);
            released.as_mut().enable();
            if self.is_available() {
                return;
            }
            released.await;
        }
    }
}

pub struct DataOperationGuard<'a>(&'a DataOperationGate);

impl Drop for DataOperationGuard<'_> {
    fn drop(&mut self) {
        self.0.occupied.store(false, Ordering::Release);
        self.0.released.notify_waiters();
    }
}

#[cfg(test)]
mod tests {
    use super::DataOperationGate;
    use std::sync::{Arc, Barrier};

    #[test]
    fn an_active_commit_blocks_cleanup_and_other_commits() {
        let gate = DataOperationGate::default();
        let check = gate.try_acquire().expect("idle commit can start");
        assert!(
            gate.try_acquire().is_none(),
            "cleanup cannot overlap a commit"
        );
        assert!(gate.try_acquire().is_none(), "second commit cannot start");
        drop(check);
        assert!(gate.try_acquire().is_some(), "cleanup can follow a check");
    }

    #[test]
    fn cleanup_blocks_a_commit_until_it_finishes() {
        let gate = DataOperationGate::default();
        let cleanup = gate.try_acquire().expect("idle cleanup can start");
        assert!(gate.try_acquire().is_none());
        drop(cleanup);
        assert!(gate.try_acquire().is_some(), "check can follow cleanup");
    }

    #[test]
    fn early_return_or_panic_does_not_leave_the_gate_locked() {
        let gate = DataOperationGate::default();
        let result = std::panic::catch_unwind(|| {
            let _guard = gate.try_acquire().unwrap();
            panic!("operation failed");
        });
        assert!(result.is_err());
        assert!(gate.try_acquire().is_some());
    }

    #[test]
    fn simultaneous_requests_have_exactly_one_winner() {
        let gate = Arc::new(DataOperationGate::default());
        let start = Arc::new(Barrier::new(3));
        let finish = Arc::new(Barrier::new(3));
        let workers: Vec<_> = (0..2)
            .map(|_| {
                let gate = Arc::clone(&gate);
                let start = Arc::clone(&start);
                let finish = Arc::clone(&finish);
                std::thread::spawn(move || {
                    start.wait();
                    let guard = gate.try_acquire();
                    let acquired = guard.is_some();
                    finish.wait();
                    drop(guard);
                    acquired
                })
            })
            .collect();
        start.wait();
        finish.wait();
        let winners = workers
            .into_iter()
            .map(|worker| usize::from(worker.join().unwrap()))
            .sum::<usize>();
        assert_eq!(winners, 1);
        assert!(gate.try_acquire().is_some());
    }
}
