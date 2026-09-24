use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const CLEANUP_RESERVE: Duration = Duration::from_secs(30);
pub const DEFAULT_STALL_TIMEOUT: Duration = Duration::from_secs(120);

/// Shared timing, progress and diagnostic context for long-running E2E tests.
pub struct E2eContext {
    name: String,
    started: Instant,
    hard_deadline: Instant,
    output_dir: Option<PathBuf>,
}

impl E2eContext {
    pub fn from_env(name: impl Into<String>) -> Self {
        let timeout = std::env::var("TEST_TIMEOUT")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .map(Duration::from_secs)
            .unwrap_or(Duration::from_secs(600));
        Self::with_timeout(name, timeout)
    }

    pub fn with_timeout(name: impl Into<String>, timeout: Duration) -> Self {
        let started = Instant::now();
        let usable = timeout
            .saturating_sub(CLEANUP_RESERVE)
            .max(Duration::from_secs(1));
        let output_dir = std::env::var_os("TEST_UNDECLARED_OUTPUTS_DIR").map(PathBuf::from);
        if let Some(directory) = &output_dir {
            let _ = fs::create_dir_all(directory);
        }
        Self {
            name: name.into(),
            started,
            hard_deadline: started + usable,
            output_dir,
        }
    }

    pub fn remaining(&self) -> Duration {
        self.hard_deadline.saturating_duration_since(Instant::now())
    }

    pub fn run_phase<T>(
        &self,
        name: &str,
        operation: impl FnOnce() -> Result<T, String>,
    ) -> Result<T, String> {
        if self.remaining().is_zero() {
            return Err(format!("e2e phase {name} started after the test deadline"));
        }
        let phase_started = Instant::now();
        eprintln!("e2e-phase: start name={name}");
        let result = operation();
        let elapsed = phase_started.elapsed();
        let status = if result.is_ok() { "pass" } else { "fail" };
        eprintln!(
            "e2e-phase: {status} name={name} elapsed_ms={}",
            elapsed.as_millis()
        );
        self.record_phase(name, status, elapsed);
        result.map_err(|error| {
            let diagnostic = format!("phase {name} failed after {elapsed:?}: {error}");
            let _ = self.write_artifact("e2e-failure.txt", diagnostic.as_bytes());
            diagnostic
        })
    }

    /// Wait for a condition while requiring explicit progress changes.
    ///
    /// The probe returns `(result, progress_generation)`. Repeated arbitrary
    /// log bytes do not count as progress; callers increment the generation
    /// only after a named milestone or observable state transition.
    pub fn wait_until<T>(
        &self,
        name: &str,
        timeout: Duration,
        poll_interval: Duration,
        stall_timeout: Duration,
        mut probe: impl FnMut() -> Result<(Option<T>, u64), String>,
    ) -> Result<T, String> {
        let started = Instant::now();
        let budget = timeout.min(self.remaining());
        let deadline = started + budget;
        let mut progress = None;
        let mut progressed_at = Instant::now();
        loop {
            let (value, current_progress) = probe()?;
            if let Some(value) = value {
                return Ok(value);
            }
            if progress != Some(current_progress) {
                progress = Some(current_progress);
                progressed_at = Instant::now();
            }
            let now = Instant::now();
            if now >= deadline {
                return Err(format!(
                    "{name} timed out after {:?}; last progress generation={current_progress}",
                    started.elapsed().min(budget)
                ));
            }
            if now.duration_since(progressed_at) >= stall_timeout {
                return Err(format!(
                    "{name} stalled for {stall_timeout:?}; last progress generation={current_progress}"
                ));
            }
            std::thread::sleep(
                poll_interval.min(deadline.saturating_duration_since(Instant::now())),
            );
        }
    }

    pub fn write_artifact(&self, name: &str, bytes: &[u8]) -> Result<Option<PathBuf>, String> {
        let Some(directory) = &self.output_dir else {
            return Ok(None);
        };
        let path = directory.join(sanitize(name));
        fs::write(&path, bytes).map_err(|error| format!("write {}: {error}", path.display()))?;
        Ok(Some(path))
    }

    pub fn output_dir(&self) -> Option<&Path> {
        self.output_dir.as_deref()
    }

    fn record_phase(&self, phase: &str, status: &str, elapsed: Duration) {
        let Some(directory) = &self.output_dir else {
            return;
        };
        let path = directory.join("e2e-phases.jsonl");
        let Ok(mut output) = OpenOptions::new().create(true).append(true).open(path) else {
            return;
        };
        let _ = writeln!(
            output,
            "{{\"test\":\"{}\",\"phase\":\"{}\",\"status\":\"{}\",\"elapsed_ms\":{},\"test_elapsed_ms\":{}}}",
            json_escape(&self.name),
            json_escape(phase),
            status,
            elapsed.as_millis(),
            self.started.elapsed().as_millis(),
        );
    }
}

fn sanitize(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .collect()
}

fn json_escape(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    #[test]
    fn wait_until_observes_progress_and_completion() {
        let context = E2eContext::with_timeout("progress", Duration::from_secs(2));
        let generation = AtomicU64::new(0);
        let value = context
            .wait_until(
                "fixture",
                Duration::from_secs(1),
                Duration::from_millis(1),
                Duration::from_millis(50),
                || {
                    let current = generation.fetch_add(1, Ordering::Relaxed);
                    Ok(((current >= 2).then_some(7), current))
                },
            )
            .unwrap();
        assert_eq!(value, 7);
    }

    #[test]
    fn wait_until_rejects_a_stall() {
        let context = E2eContext::with_timeout("stall", Duration::from_secs(2));
        let error = context
            .wait_until::<()>(
                "fixture",
                Duration::from_secs(1),
                Duration::from_millis(1),
                Duration::from_millis(5),
                || Ok((None, 1)),
            )
            .unwrap_err();
        assert!(error.contains("stalled"), "{error}");
    }

    #[test]
    fn artifact_names_are_bounded_to_one_directory() {
        assert_eq!(sanitize("serial/../tail.log"), "serial_.._tail.log");
    }
}
