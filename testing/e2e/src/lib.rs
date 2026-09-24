use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;
use std::time::Instant;

use bexos_debug_client::{DebugClient, DebugClientError, DebugTransport};
use bexos_trace::{BufferMode, TraceOutputFormat};
pub use bexos_trace_analysis::TraceAnalysis;

mod context;
pub use context::{DEFAULT_STALL_TIMEOUT, E2eContext};

pub const BOOT_MARKERS: &[&[u8]] = &[
    b"userspace: entering el0 appd",
    b"pl011: EL0 driver ready",
    b"/pkg mounted read-only",
    b"pivot complete; /boot removed; BootFS reclaimed pages=",
    b"reclaimed physical pages reused=",
    b"appd: guest persistence and disk-only application verified",
];

pub fn boot_markers() -> Vec<&'static [u8]> {
    boot_markers_for_arch(std::env::var("BEXOS_QEMU_ARCH").as_deref() == Ok("x86_64"))
}

pub fn boot_markers_for_arch(x86_64: bool) -> Vec<&'static [u8]> {
    if x86_64 {
        let mut markers = vec![b"userspace: entering ring3 appd".as_slice()];
        markers.extend_from_slice(&BOOT_MARKERS[2..]);
        markers
    } else {
        BOOT_MARKERS.to_vec()
    }
}

pub const NVME_MARKERS: &[&[u8]] = &[b"pci: EL0 enumeration", b"nvme: EL0 controller identified"];

pub const VIRTIO_NET_MARKERS: &[&[u8]] = &[b"virtio-net: EL0 device ready mac=52:54:00:12:34:56"];

pub const USB_MARKERS: &[&[u8]] = &[b"xhcid: xHCI caps", b"usbd: ready"];

pub const BEXFS_MARKERS: &[&[u8]] = &[
    b"bexfs: EL0 filesystem service ready",
    b"guest SYS_STATE durable generation=",
];

pub trait E2eDevice {
    type DebugTransport: DebugTransport;

    fn boot(&mut self, markers: &[&[u8]]) -> Result<Vec<u8>, String>;
    fn boot_with_debugd(
        &mut self,
        markers: &[&[u8]],
    ) -> Result<DebugSession<Self::DebugTransport>, String>;
    fn inspect_path(
        &self,
        partition: &str,
        label: &str,
        path: &str,
        extra: &[&str],
    ) -> Result<(), String>;
}

pub struct DebugSession<T: DebugTransport> {
    pub boot_output: Vec<u8>,
    pub client: DebugClient<T>,
    _guard: Option<Box<dyn DebugSessionGuard>>,
}

impl<T: DebugTransport> DebugSession<T> {
    pub fn new(
        boot_output: Vec<u8>,
        client: DebugClient<T>,
        guard: Option<Box<dyn DebugSessionGuard>>,
    ) -> Self {
        Self {
            boot_output,
            client,
            _guard: guard,
        }
    }
}

pub trait DebugSessionGuard {}

impl<T: DebugSessionGuard + ?Sized> DebugSessionGuard for Box<T> {}

impl<T: DebugTransport> Drop for DebugSession<T> {
    fn drop(&mut self) {
        let _ = self._guard.take();
    }
}

impl<T: DebugTransport> DebugSession<T> {
    pub fn assert_debugd_ready(&mut self) -> Result<(), String> {
        eprintln!("e2e: debugd health check");
        let health = self.client.health_check().map_err(|e| {
            format!(
                "{}\n{}",
                format_debug_error(e),
                String::from_utf8_lossy(self.client.received_trace())
            )
        })?;
        if health.service_name != "debugd" || health.status != "SERVING" {
            return Err(format!("unexpected debugd health response: {health:?}"));
        }
        eprintln!("e2e: debugd list processes");
        let processes = self.client.list_processes().map_err(|e| {
            format!(
                "{}\n{}",
                format_debug_error(e),
                String::from_utf8_lossy(self.client.received_trace())
            )
        })?;
        if processes.is_empty() {
            return Err("debugd returned an empty process list".into());
        }
        eprintln!("e2e: debugd version");
        let response = self
            .client
            .exec_command("debugd.version", &[])
            .map_err(|e| {
                format!(
                    "{}\n{}",
                    format_debug_error(e),
                    String::from_utf8_lossy(self.client.received_trace())
                )
            })?;
        if response.exit_code != 0 || !response.stdout.contains("qemu-socket-v1") {
            return Err(format!("debugd.version failed: {response:?}"));
        }
        Ok(())
    }

    pub fn install_and_launch_test_app(
        &mut self,
        package_id: &str,
        process_name: &str,
        manifest: impl AsRef<Path>,
        elf: impl AsRef<Path>,
        arg0: u64,
    ) -> Result<(), String> {
        let manifest = fs::read(manifest.as_ref()).map_err(|e| format!("read manifest: {e}"))?;
        let elf = fs::read(elf.as_ref()).map_err(|e| format!("read ELF: {e}"))?;
        let upload_id = manifest.len() as u64 ^ ((elf.len() as u64) << 17) ^ 0xbe05;
        self.client
            .install_test_app(upload_id, package_id, &manifest, &elf)
            .map_err(format_debug_error)?;
        self.client
            .launch_test_app(package_id, process_name, arg0)
            .map_err(format_debug_error)
    }

    pub fn start_trace(
        &mut self,
        categories: u32,
        buffer_mode: BufferMode,
        buffer_size_kb: u32,
    ) -> Result<(), String> {
        self.start_trace_with_format(
            categories,
            buffer_mode,
            buffer_size_kb,
            TraceOutputFormat::Perfetto,
        )
    }

    pub fn start_trace_with_format(
        &mut self,
        categories: u32,
        buffer_mode: BufferMode,
        buffer_size_kb: u32,
        output_format: TraceOutputFormat,
    ) -> Result<(), String> {
        self.client
            .trace_start_with_format(
                categories,
                buffer_mode.to_wire(),
                buffer_size_kb,
                output_format.to_wire(),
            )
            .map_err(format_debug_error)
    }

    pub fn stop_trace_to_file(&mut self, path: impl AsRef<Path>) -> Result<PathBuf, String> {
        let bytes = self.client.trace_stop().map_err(format_debug_error)?;
        if !bexos_trace::looks_like_perfetto_trace(&bytes)
            && !bexos_trace::looks_like_legacy_bexos_fxt(&bytes)
        {
            return Err("trace was neither Perfetto protobuf nor legacy BexOS FXT".into());
        }
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("create trace dir: {e}"))?;
        }
        fs::write(path, &bytes).map_err(|e| format!("write trace: {e}"))?;
        Ok(path.to_path_buf())
    }

    pub fn stop_trace_to_analysis(
        &mut self,
        path: impl AsRef<Path>,
    ) -> Result<TraceAnalysis, String> {
        let path = self.stop_trace_to_file(path)?;
        TraceAnalysis::from_file(path)
    }

    pub fn traced_test<'a>(
        &'a mut self,
        test_name: &str,
        categories: u32,
    ) -> Result<TracedTestGuard<'a, T>, String> {
        self.start_trace(categories, BufferMode::CircularRing, 2048)?;
        Ok(TracedTestGuard {
            session: self,
            path: default_trace_path(test_name),
            collected: false,
        })
    }

    pub fn wait_for_serial_markers(
        &mut self,
        markers: &[&[u8]],
        timeout: Duration,
    ) -> Result<Vec<u8>, String> {
        self.wait_for_serial_markers_observed(markers, timeout, |_| {})
    }

    pub fn wait_for_serial_markers_observed(
        &mut self,
        markers: &[&[u8]],
        timeout: Duration,
        mut observe: impl FnMut(&[u8]),
    ) -> Result<Vec<u8>, String> {
        let deadline = Instant::now() + timeout;
        let mut progress = 0;
        let mut progressed_at = Instant::now();
        let mut output = self.combined_serial_output();
        while Instant::now() < deadline {
            observe(&output);
            assert_absent_with_tail(&output, b"panic", "guest panic during traced QEMU boot")?;
            assert_absent_with_tail(
                &output,
                b"guest fault",
                "guest fault during traced QEMU boot",
            )?;
            assert_absent_with_tail(&output, b"boot failed:", "guest boot failed")?;
            let current_progress = markers
                .iter()
                .filter(|marker| contains(&output, marker))
                .count();
            if current_progress == markers.len() {
                return Ok(output);
            }
            if current_progress != progress {
                progress = current_progress;
                progressed_at = Instant::now();
                eprintln!(
                    "e2e: marker progress {progress}/{} elapsed_ms={}",
                    markers.len(),
                    timeout
                        .saturating_sub(deadline.saturating_duration_since(Instant::now()))
                        .as_millis(),
                );
            }
            if progressed_at.elapsed() >= DEFAULT_STALL_TIMEOUT {
                let missing = markers
                    .iter()
                    .filter(|marker| !contains(&output, marker))
                    .map(|marker| String::from_utf8_lossy(marker).into_owned())
                    .collect::<Vec<_>>();
                return Err(format!(
                    "stalled for {DEFAULT_STALL_TIMEOUT:?} waiting for serial markers: {}\n{}",
                    missing.join(", "),
                    serial_tail(&output),
                ));
            }
            self.client
                .drain_for(Duration::from_millis(250))
                .map_err(format_debug_error)?;
            output = self.combined_serial_output();
        }
        assert_markers(&output, markers)?;
        Err("timed out waiting for traced QEMU boot markers".into())
    }

    fn combined_serial_output(&self) -> Vec<u8> {
        let mut output = self.boot_output.clone();
        output.extend_from_slice(self.client.received_trace());
        output
    }
}

pub struct TracedTestGuard<'a, T: DebugTransport> {
    session: &'a mut DebugSession<T>,
    path: PathBuf,
    collected: bool,
}

impl<T: DebugTransport> TracedTestGuard<'_, T> {
    pub fn session(&mut self) -> &mut DebugSession<T> {
        self.session
    }

    pub fn finish(mut self) -> Result<TraceAnalysis, String> {
        let path = self.session.stop_trace_to_file(&self.path)?;
        self.collected = true;
        TraceAnalysis::from_file(path)
    }
}

impl<T: DebugTransport> Drop for TracedTestGuard<'_, T> {
    fn drop(&mut self) {
        if !self.collected {
            let _ = self.session.stop_trace_to_file(&self.path);
        }
    }
}

pub fn default_trace_path(test_name: &str) -> PathBuf {
    let sanitized = test_name
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();
    if let Ok(dir) = std::env::var("TEST_UNDECLARED_OUTPUTS_DIR") {
        return PathBuf::from(dir).join(format!("{sanitized}.pftrace"));
    }
    PathBuf::from("target/traces").join(format!("{sanitized}.pftrace"))
}

pub fn assert_markers(output: &[u8], markers: &[&[u8]]) -> Result<(), String> {
    let missing = markers
        .iter()
        .copied()
        .filter(|marker| !output.windows(marker.len()).any(|window| window == *marker))
        .map(|marker| String::from_utf8_lossy(marker).into_owned())
        .collect::<Vec<_>>();
    if missing.is_empty() {
        Ok(())
    } else {
        Err(format!("missing boot markers: {}", missing.join(", ")))
    }
}

pub fn contains(output: &[u8], marker: &[u8]) -> bool {
    output.windows(marker.len()).any(|window| window == marker)
}

fn assert_absent_with_tail(output: &[u8], marker: &[u8], message: &str) -> Result<(), String> {
    if contains(output, marker) {
        Err(format!("{message}\n{}", serial_tail(output)))
    } else {
        Ok(())
    }
}

pub fn assert_absent(output: &[u8], marker: &[u8], message: &str) -> Result<(), String> {
    if contains(output, marker) {
        Err(message.into())
    } else {
        Ok(())
    }
}

fn serial_tail(output: &[u8]) -> String {
    const MAX_TAIL: usize = 8192;
    let start = output.len().saturating_sub(MAX_TAIL);
    String::from_utf8_lossy(&output[start..]).into_owned()
}

pub fn assert_fxt_trace(bytes: &[u8]) -> Result<(), String> {
    if bexos_trace::looks_like_legacy_bexos_fxt(bytes) {
        Ok(())
    } else {
        Err("trace did not have BexOS FXT header".into())
    }
}

pub fn assert_perfetto_trace(bytes: &[u8]) -> Result<(), String> {
    if bexos_trace::looks_like_perfetto_trace(bytes) {
        Ok(())
    } else {
        Err("trace did not have Perfetto protobuf header".into())
    }
}

pub fn assert_trace_event(bytes: &[u8], event_name: &str) -> Result<(), String> {
    if !bexos_trace::looks_like_perfetto_trace(bytes)
        && !bexos_trace::looks_like_legacy_bexos_fxt(bytes)
    {
        return Err("trace was neither Perfetto protobuf nor legacy BexOS FXT".into());
    }
    let event = event_name.as_bytes();
    if bytes.windows(event.len()).any(|window| window == event) {
        Ok(())
    } else {
        Err(format!("trace did not contain event {event_name}"))
    }
}

pub fn assert_trace_file_event(path: impl AsRef<Path>, event_name: &str) -> Result<(), String> {
    let path = path.as_ref();
    let bytes = fs::read(path).map_err(|e| format!("read trace {}: {e}", path.display()))?;
    assert_trace_event(&bytes, event_name)
}

pub fn format_debug_error(error: DebugClientError) -> String {
    format!("{error:?}")
}
