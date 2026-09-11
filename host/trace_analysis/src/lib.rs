use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[derive(Clone, Debug)]
pub struct TraceAnalysis {
    trace_path: PathBuf,
    bytes: Vec<u8>,
    trace_processor: Option<PathBuf>,
}

impl TraceAnalysis {
    pub fn from_file(path: impl AsRef<Path>) -> Result<Self, String> {
        let path = path.as_ref();
        let bytes = fs::read(path).map_err(|e| format!("read trace {}: {e}", path.display()))?;
        Self::from_bytes(path.to_path_buf(), bytes)
    }

    pub fn from_bytes(trace_path: PathBuf, bytes: Vec<u8>) -> Result<Self, String> {
        let looks_like_bexos_perfetto = bexos_trace::looks_like_perfetto_trace(&bytes)
            && (contains(&bytes, b"bexos-process") || contains(&bytes, b"bexos-thread"));
        if !looks_like_bexos_perfetto && !bexos_trace::looks_like_legacy_bexos_fxt(&bytes) {
            return Err("trace is neither Perfetto protobuf nor legacy BexOS FXT".into());
        }
        Ok(Self {
            trace_path,
            bytes,
            trace_processor: trace_processor_from_env(),
        })
    }

    pub fn with_trace_processor(mut self, trace_processor: impl Into<PathBuf>) -> Self {
        self.trace_processor = Some(trace_processor.into());
        self
    }

    pub fn query(&self, sql: &str) -> Result<String, String> {
        let processor = self
            .trace_processor
            .as_ref()
            .ok_or("BEXOS_TRACE_PROCESSOR is not set; SQL queries require trace_processor_shell")?;
        let output = Command::new(processor)
            .arg("--query-string")
            .arg(sql)
            .arg(&self.trace_path)
            .stdin(Stdio::null())
            .output()
            .map_err(|e| format!("run trace processor: {e}"))?;
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).into_owned())
        } else {
            Err(String::from_utf8_lossy(&output.stderr).into_owned())
        }
    }

    pub fn scalar_i64(&self, sql: &str) -> Result<i64, String> {
        let output = self.query(sql)?;
        output
            .split(|ch: char| ch == ',' || ch == '\n' || ch == '\t' || ch == ' ')
            .find_map(|part| part.trim().parse::<i64>().ok())
            .ok_or_else(|| format!("query did not return an integer scalar: {output}"))
    }

    pub fn assert_event_present(&self, event_name: &str) -> Result<(), String> {
        self.assert_bytes_contain("event", event_name)
    }

    pub fn assert_category_present(&self, category_name: &str) -> Result<(), String> {
        self.assert_bytes_contain("category", category_name)
    }

    pub fn assert_flow_count_at_least(&self, expected: usize) -> Result<(), String> {
        let count = self.count_any(&["flow_id", "flow"]);
        if count >= expected {
            Ok(())
        } else {
            Err(format!(
                "expected at least {expected} flow markers, found {count}"
            ))
        }
    }

    pub fn assert_counter_present(&self, counter_name: &str) -> Result<(), String> {
        self.assert_bytes_contain("counter", counter_name)
    }

    pub fn assert_no_dropped_events(&self) -> Result<(), String> {
        if self
            .bytes
            .windows(b"dropped_events".len())
            .any(|w| w == b"dropped_events")
        {
            return Err("trace contains a dropped_events marker".into());
        }
        Ok(())
    }

    pub fn assert_slice_duration_at_least_ns(
        &self,
        slice_name: &str,
        min_duration_ns: i64,
    ) -> Result<(), String> {
        if self.trace_processor.is_some() {
            let sql = format!(
                "select max(dur) from slice where name = '{}'",
                slice_name.replace('\'', "''")
            );
            let duration = self.scalar_i64(&sql)?;
            if duration >= min_duration_ns {
                return Ok(());
            }
            return Err(format!(
                "slice {slice_name} max duration {duration}ns was below {min_duration_ns}ns"
            ));
        }
        self.assert_event_present(slice_name)
    }

    fn assert_bytes_contain(&self, kind: &str, needle: &str) -> Result<(), String> {
        let needle = needle.as_bytes();
        if self
            .bytes
            .windows(needle.len())
            .any(|window| window == needle)
        {
            Ok(())
        } else {
            Err(format!("trace did not contain {kind} {needle:?}"))
        }
    }

    fn count_any(&self, needles: &[&str]) -> usize {
        needles
            .iter()
            .map(|needle| {
                self.bytes
                    .windows(needle.len())
                    .filter(|window| *window == needle.as_bytes())
                    .count()
            })
            .sum()
    }
}

fn trace_processor_from_env() -> Option<PathBuf> {
    std::env::var_os("BEXOS_TRACE_PROCESSOR")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}
