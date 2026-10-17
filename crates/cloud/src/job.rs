//! Background jobs: network work runs on its own thread while the UI keeps drawing. A job reports
//! progress (a fraction and a message), can be cancelled (the work checks [`Cancel`] between
//! steps and stops with [`CANCELLED`]), and hands back its result once.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

/// The error a job's work returns when it stopped because it was cancelled.
pub const CANCELLED: &str = "cancelled";

/// A job's cancel flag, shared with its work.
#[derive(Clone, Debug, Default)]
pub struct Cancel(Arc<AtomicBool>);

impl Cancel {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }

    /// `Err(CANCELLED)` once cancelled: `cancel.check()?` between steps.
    pub fn check(&self) -> Result<(), String> {
        if self.is_cancelled() { Err(CANCELLED.into()) } else { Ok(()) }
    }
}

/// A job's progress, shared with its work: 0–1 and what it is doing.
#[derive(Clone, Debug, Default)]
pub struct Progress(Arc<Mutex<(f32, String)>>);

impl Progress {
    pub fn set(&self, fraction: f32, message: impl Into<String>) {
        if let Ok(mut p) = self.0.lock() {
            *p = (if fraction.is_finite() { fraction.clamp(0.0, 1.0) } else { 0.0 }, message.into());
        }
    }

    pub fn get(&self) -> (f32, String) {
        self.0.lock().map(|p| p.clone()).unwrap_or_default()
    }
}

/// How a job ended.
#[derive(Debug, PartialEq)]
pub enum Outcome<T> {
    Done(T),
    Failed(String),
    Cancelled,
}

/// Work running in the background.
pub struct Job<T> {
    rx: mpsc::Receiver<Result<T, String>>,
    progress: Progress,
    cancel: Cancel,
    ended: bool,
}

impl<T: Send + 'static> Job<T> {
    /// Run `work` on a new thread. On the web, where there are no threads, the job fails at once.
    pub fn spawn(name: &str, work: impl FnOnce(&Progress, &Cancel) -> Result<T, String> + Send + 'static) -> Self {
        let (tx, rx) = mpsc::channel();
        let (progress, cancel) = (Progress::default(), Cancel::default());
        #[cfg(not(target_arch = "wasm32"))]
        {
            let (p, c) = (progress.clone(), cancel.clone());
            let failed = tx.clone();
            let spawned = std::thread::Builder::new().name(format!("dlightroom-{name}")).spawn(move || {
                // A panic in the work is a failure of the job, never of the app.
                let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| work(&p, &c)))
                    .unwrap_or_else(|_| Err("the job stopped unexpectedly".into()));
                // The receiver may be gone (the job was dropped): nothing to tell then.
                let _ = tx.send(r);
            });
            if let Err(e) = spawned {
                // The receiver is right here: this send can't fail.
                let _ = failed.send(Err(format!("couldn't start {name}: {e}")));
            }
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = (name, work);
            let _ = tx.send(Err("this needs the desktop app".into()));
        }
        Self { rx, progress, cancel, ended: false }
    }

    /// A job that has already ended with `r` (for callers that must hand back a job either way).
    pub fn finished(r: Result<T, String>) -> Self {
        let (tx, rx) = mpsc::channel();
        let _ = tx.send(r);
        Self { rx, progress: Progress::default(), cancel: Cancel::default(), ended: false }
    }

    pub fn progress(&self) -> (f32, String) {
        self.progress.get()
    }

    pub fn cancel(&self) {
        self.cancel.cancel();
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancel.is_cancelled()
    }

    /// The outcome, once, when the work has ended; `None` while it runs (and after the outcome
    /// was taken).
    pub fn poll(&mut self) -> Option<Outcome<T>> {
        if self.ended {
            return None;
        }
        let r = match self.rx.try_recv() {
            Ok(r) => r,
            Err(mpsc::TryRecvError::Empty) => return None,
            Err(mpsc::TryRecvError::Disconnected) => Err("the job stopped unexpectedly".into()),
        };
        self.ended = true;
        Some(match r {
            Ok(v) => Outcome::Done(v),
            Err(e) if e == CANCELLED || self.cancel.is_cancelled() => Outcome::Cancelled,
            Err(e) => Outcome::Failed(e),
        })
    }

    /// Wait for the outcome (tests and command-line use).
    pub fn wait(mut self) -> Outcome<T> {
        if self.ended {
            return Outcome::Failed("the outcome was already taken".into());
        }
        let r = self.rx.recv().unwrap_or_else(|_| Err("the job stopped unexpectedly".into()));
        self.ended = true;
        match r {
            Ok(v) => Outcome::Done(v),
            Err(e) if e == CANCELLED || self.cancel.is_cancelled() => Outcome::Cancelled,
            Err(e) => Outcome::Failed(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn a_job_reports_progress_and_its_result_once() {
        let mut job = Job::spawn("test", |p, _| {
            p.set(0.5, "half");
            Ok(42)
        });
        let mut outcome = None;
        for _ in 0..500 {
            if let Some(o) = job.poll() {
                outcome = Some(o);
                break;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(outcome, Some(Outcome::Done(42)));
        assert_eq!(job.progress(), (0.5, "half".to_string()));
        assert_eq!(job.poll(), None, "only once");
    }

    #[test]
    fn cancelling_stops_the_work_between_steps() {
        let job = Job::spawn("test", |_, c| {
            for _ in 0..1000 {
                c.check()?;
                std::thread::sleep(Duration::from_millis(1));
            }
            Ok(())
        });
        job.cancel();
        assert_eq!(job.wait(), Outcome::Cancelled);
    }

    #[test]
    fn failures_and_panics_are_reported() {
        assert_eq!(Job::<()>::spawn("test", |_, _| Err("no network".into())).wait(), Outcome::Failed("no network".into()));
        assert!(matches!(Job::<()>::spawn("test", |_, _| panic!("boom")).wait(), Outcome::Failed(_)));
        assert_eq!(Job::finished(Ok(7)).wait(), Outcome::Done(7));
    }
}
