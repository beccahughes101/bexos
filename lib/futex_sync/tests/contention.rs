use bexos_futex_sync::*;
use std::sync::{
    Arc, Barrier, Condvar, Mutex,
    atomic::{AtomicU32, AtomicU64, Ordering},
};
#[derive(Default)]
struct Host {
    gate: Mutex<()>,
    changed: Condvar,
}
impl Wait for Host {
    fn wait(&self, word: &AtomicU32, expected: u32) -> Result<(), i32> {
        let mut gate = self.gate.lock().unwrap();
        while word.load(Ordering::Relaxed) == expected {
            let (next, timeout) = self
                .changed
                .wait_timeout(gate, std::time::Duration::from_secs(5))
                .unwrap();
            gate = next;
            assert!(!timeout.timed_out(), "lost wakeup");
        }
        Ok(())
    }
    fn wake(&self, _: &AtomicU32, count: u32) -> Result<(), i32> {
        let _gate = self.gate.lock().unwrap();
        if count == 1 {
            self.changed.notify_one();
        } else {
            self.changed.notify_all();
        }
        Ok(())
    }
}
#[test]
fn contended_mutex_publishes_writes_and_once_initializes_exactly_once() {
    let shared = Arc::new((
        AtomicU32::new(0),
        AtomicU64::new(0),
        Host::default(),
        AtomicU32::new(0),
        AtomicU32::new(0),
        Barrier::new(8),
    ));
    let threads: Vec<_> = (0..8)
        .map(|_| {
            let s = shared.clone();
            std::thread::spawn(move || {
                s.5.wait();
                once(&s.3, &s.2, || {
                    s.4.fetch_add(1, Ordering::Relaxed);
                    std::thread::yield_now();
                })
                .unwrap();
                for _ in 0..2000 {
                    lock(&s.0, &s.2).unwrap();
                    let count = s.1.load(Ordering::Relaxed);
                    std::thread::yield_now();
                    s.1.store(count + 1, Ordering::Relaxed);
                    unlock(&s.0, &s.2).unwrap();
                }
            })
        })
        .collect();
    for thread in threads {
        thread.join().unwrap();
    }
    assert_eq!(shared.1.load(Ordering::Relaxed), 16000);
    assert_eq!(shared.4.load(Ordering::Relaxed), 1);
    destroy(&shared.0).unwrap();
    assert_eq!(try_lock(&shared.0), Err(INVALID));
}
#[test]
fn permanent_wait_failure_does_not_report_acquisition_or_corrupt_owner_state() {
    struct Failed;
    impl Wait for Failed {
        fn wait(&self, _: &AtomicU32, _: u32) -> Result<(), i32> {
            Err(38)
        }
        fn wake(&self, _: &AtomicU32, _: u32) -> Result<(), i32> {
            Ok(())
        }
    }
    let state = AtomicU32::new(0);
    try_lock(&state).unwrap();
    assert_eq!(try_lock(&state), Err(BUSY));
    assert_eq!(destroy(&state), Err(BUSY));
    assert_eq!(lock(&state, &Failed), Err(38));
    assert_eq!(state.load(Ordering::Relaxed), 2);
    unlock(&state, &Failed).unwrap();
    assert_eq!(unlock(&state, &Failed), Err(INVALID));
}

#[test]
fn condition_broadcast_releases_all_waiters_and_reacquires_mutex() {
    use condition::Condition;
    let shared = Arc::new((
        AtomicU32::new(0),
        Condition::default(),
        Host::default(),
        Host::default(),
        AtomicU32::new(0),
        AtomicU32::new(0),
    ));
    let workers: Vec<_> = (0..8)
        .map(|_| {
            let s = shared.clone();
            std::thread::spawn(move || {
                lock(&s.0, &s.2).unwrap();
                s.4.fetch_add(1, Ordering::Relaxed);
                while s.5.load(Ordering::Relaxed) == 0 {
                    s.1.wait(&s.0, &s.3, &s.2).unwrap();
                }
                assert_eq!(try_lock(&s.0), Err(BUSY));
                unlock(&s.0, &s.2).unwrap();
            })
        })
        .collect();
    loop {
        lock(&shared.0, &shared.2).unwrap();
        if shared.4.load(Ordering::Relaxed) == 8 {
            break;
        }
        unlock(&shared.0, &shared.2).unwrap();
        std::thread::yield_now();
    }
    assert_eq!(shared.1.destroy(), Err(BUSY));
    shared.5.store(1, Ordering::Relaxed);
    shared.1.signal(true, &shared.3).unwrap();
    unlock(&shared.0, &shared.2).unwrap();
    for worker in workers {
        worker.join().unwrap();
    }
    shared.1.destroy().unwrap();
    assert_eq!(shared.1.signal(false, &shared.3), Err(INVALID));
}

#[test]
fn condition_timeout_reacquires_and_signal_between_unlock_and_sleep_is_not_lost() {
    use condition::Condition;
    struct Timeout;
    impl Wait for Timeout {
        fn wait(&self, _: &AtomicU32, _: u32) -> Result<(), i32> {
            Err(110)
        }
        fn wake(&self, _: &AtomicU32, _: u32) -> Result<(), i32> {
            Ok(())
        }
    }
    let mutex = AtomicU32::new(0);
    let condition = Condition::default();
    lock(&mutex, &Timeout).unwrap();
    assert_eq!(condition.wait(&mutex, &Timeout, &Timeout), Err(110));
    assert_eq!(try_lock(&mutex), Err(BUSY));
    struct RacingSignal<'a>(&'a Condition, &'a AtomicU32);
    impl Wait for RacingSignal<'_> {
        fn wait(&self, word: &AtomicU32, expected: u32) -> Result<(), i32> {
            assert_eq!(
                self.1.load(Ordering::Acquire),
                0,
                "condition sleep must release the mutex"
            );
            self.0.signal(false, &Timeout).unwrap();
            assert_ne!(
                word.load(Ordering::Acquire),
                expected,
                "futex must observe the raced signal"
            );
            Ok(())
        }
        fn wake(&self, _: &AtomicU32, _: u32) -> Result<(), i32> {
            Ok(())
        }
    }
    condition
        .wait(&mutex, &RacingSignal(&condition, &mutex), &Timeout)
        .unwrap();
    assert_eq!(try_lock(&mutex), Err(BUSY));
    unlock(&mutex, &Timeout).unwrap();
    condition.destroy().unwrap();
}
