//! Process-local nonoverlapping completion windows. Logical frame timestamps
//! migrate; diagnostic counters restart, with 32 warmup completions.
use bexos_graphics::metrics::Window;
use bexos_libc::allocation_stats::{self, Snapshot};
#[derive(Clone, Copy)]
pub struct Sample {
    wall_us: u64,
    cpu: Option<(u64, u64)>,
    allocations: Snapshot,
}
impl Sample {
    pub fn capture() -> Self {
        let cpu = bexos_userspace::syscall::runtime_stats().ok();
        Self {
            wall_us: bexos_graphics_runtime::now_us(),
            cpu,
            allocations: allocation_stats::snapshot(),
        }
    }
}
pub struct WorkerSample {
    pub wall_us: u64,
    pub cpu_ns: Option<u64>,
    pub submission_us: u64,
    pub interval_ns: Option<u64>,
    pub readback_bytes: u64,
}
#[derive(Default)]
pub struct Metrics {
    pub pending_worker: Option<WorkerSample>,
    pub pending_cpu_bytes: u64,
    pub main_wall_us: Window,
    pub main_cpu_ns: Window,
    pub process_cpu_ns: Window,
    pub worker_cpu_ns: Window,
    pub worker_wall_us: Window,
    pub processing_us: Window,
    pub dispatch_lateness_us: Window,
    pub submission_us: Window,
    pub gpu_interval_ns: Window,
    pub completed: u64,
    pub measured: u64,
    pub failed: u64,
    pub missed_timer_deadlines: u64,
    pub cpu_output_bytes: u64,
    pub direct_frames: u64,
    pub gpu_frames: u64,
    pub gpu_readback_bytes: u64,
    pub workload: u32,
    epoch_us: u64,
    window_id: u64,
    baseline: Option<Sample>,
    allocations: Snapshot,
    main_wall_total: u64,
    completion_pending: bool,
    report_pending: bool,
}
impl Metrics {
    pub fn measurement(workload: u32) -> Self {
        Self {
            workload,
            epoch_us: bexos_graphics_runtime::now_us(),
            ..Self::default()
        }
    }

    pub fn cancel_pending(&mut self) {
        self.pending_worker = None;
        self.pending_cpu_bytes = 0;
    }

    pub fn complete(&mut self, now: u64, frame: &crate::presentation::PendingFrame) {
        let worker = self.pending_worker.take();
        let cpu_bytes = core::mem::take(&mut self.pending_cpu_bytes);
        self.completed = self.completed.saturating_add(1);
        self.completion_pending = true;
        if self.completed <= 32 {
            return;
        }
        self.measured = self.measured.saturating_add(1);
        self.cpu_output_bytes = self.cpu_output_bytes.saturating_add(cpu_bytes);
        if let Some(worker) = worker {
            self.gpu_readback_bytes += worker.readback_bytes;
            self.gpu_frames += 1;
            self.worker_wall_us.record(worker.wall_us);
            if let Some(cpu) = worker.cpu_ns {
                self.worker_cpu_ns.record(cpu);
            }
            self.submission_us.record(worker.submission_us);
            if let Some(interval) = worker.interval_ns {
                self.gpu_interval_ns.record(interval);
            }
        }
        self.processing_us
            .record(now.saturating_sub(frame.frame_time_us));
        self.missed_timer_deadlines +=
            u64::from(frame.timer_deadline_us != 0 && now > frame.timer_deadline_us);
        self.direct_frames += u64::from(frame.scanout_buffer != 0);
        self.report_pending = self.measured % 256 == 0;
    }
    pub fn end_loop(&mut self, begin: Sample) {
        self.end_loop_at(begin, Sample::capture());
    }
    fn end_loop_at(&mut self, begin: Sample, end: Sample) {
        if self.epoch_us == 0 {
            self.epoch_us = begin.wall_us;
        }
        self.main_wall_total = self
            .main_wall_total
            .saturating_add(end.wall_us.saturating_sub(begin.wall_us));
        if !self.completion_pending {
            return;
        }
        self.completion_pending = false;
        if self.completed > 32 {
            self.main_wall_us.record(self.main_wall_total);
            if let Some(before) = self.baseline {
                if let Some((a, b)) = before.cpu.zip(end.cpu) {
                    self.main_cpu_ns.record(b.0.saturating_sub(a.0));
                    self.process_cpu_ns.record(b.1.saturating_sub(a.1));
                }
                let d = end.allocations.since(before.allocations);
                self.allocations.allocations += d.allocations;
                self.allocations.reallocations += d.reallocations;
                self.allocations.frees += d.frees;
                self.allocations.requested_bytes += d.requested_bytes;
                self.allocations.failures += d.failures;
            }
        } else {
            self.reset_windows();
        }
        self.main_wall_total = 0;
        self.baseline = Some(end);
    }
    fn reset_windows(&mut self) {
        self.main_wall_us = Window::default();
        self.main_cpu_ns = Window::default();
        self.process_cpu_ns = Window::default();
        self.worker_cpu_ns = Window::default();
        self.worker_wall_us = Window::default();
        self.processing_us = Window::default();
        self.dispatch_lateness_us = Window::default();
        self.submission_us = Window::default();
        self.gpu_interval_ns = Window::default();
        self.allocations = Snapshot::default();
        self.failed = 0;
        self.missed_timer_deadlines = 0;
        self.cpu_output_bytes = 0;
        self.direct_frames = 0;
        self.gpu_frames = 0;
        self.gpu_readback_bytes = 0;
    }
    pub fn report(&mut self) {
        if !self.report_pending {
            return;
        }
        self.report_pending = false;
        self.window_id += 1;
        use core::fmt::Write;
        let before = Sample::capture();
        let p99 = |w: &Window| crate::metrics_json::Number(w.percentile(99));
        let mut line = crate::metrics_json::Line::new();
        write!(&mut line,
            "scened-metrics: {{\"schema\":3,\"workload\":{},\"epoch_us\":{},\"window_id\":{},\"epoch_frames\":{},\"warmup\":32,\"window\":256,\"samples\":{},\"main_wall_p99_us\":{},\"main_cpu_p99_ns\":{},\"process_cpu_p99_ns\":{},\"cpu_samples\":{},\"worker_cpu_p99_ns\":{},\"worker_cpu_samples\":{},\"worker_wall_p99_us\":{},\"processing_p99_us\":{},\"dispatch_lateness_p99_us\":{},\"missed_timer_deadlines\":{},\"failures\":{},\"direct_frames\":{},\"gpu_frames\":{},\"gpu_readback_bytes\":{},\"cpu_output_bytes\":{},\"allocations\":{},\"reallocations\":{},\"allocation_bytes\":{},\"allocation_failures\":{},\"gpu_submission_p99_us\":{},\"gpu_queue_interval_p99_ns\":{},\"gpu_timestamp_samples\":{},\"hardware_vsync\":false,\"hardware_performance_verified\":false}}\n",
            self.workload,
            self.epoch_us,
            self.window_id,
            self.measured,
            self.processing_us.len(),
            p99(&self.main_wall_us),
            p99(&self.main_cpu_ns),
            p99(&self.process_cpu_ns),
            self.process_cpu_ns.len(),
            p99(&self.worker_cpu_ns),
            self.worker_cpu_ns.len(),
            p99(&self.worker_wall_us),
            p99(&self.processing_us),
            p99(&self.dispatch_lateness_us),
            self.missed_timer_deadlines,
            self.failed,
            self.direct_frames,
            self.gpu_frames,
            self.gpu_readback_bytes,
            self.cpu_output_bytes,
            self.allocations.allocations,
            self.allocations.reallocations,
            self.allocations.requested_bytes,
            self.allocations.failures,
            p99(&self.submission_us),
            p99(&self.gpu_interval_ns),
            self.gpu_interval_ns.len(),
        ).expect("bounded compositor metrics line");
        bexos_userspace::log(line.as_str());
        self.reset_windows();
        // Reporting is outside all workload samples, including process/heap deltas.
        let after = Sample::capture();
        if let Some(baseline) = &mut self.baseline {
            if let Some((a, b)) = before.cpu.zip(after.cpu) {
                // Remove only this thread's reporting CPU; concurrent worker
                // execution remains in the following process interval.
                if let Some(cpu) = &mut baseline.cpu {
                    cpu.0 = cpu.0.saturating_add(b.0.saturating_sub(a.0));
                    cpu.1 = cpu.1.saturating_add(b.0.saturating_sub(a.0));
                }
            } else {
                baseline.cpu = None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn sample(n: u64) -> Sample {
        Sample {
            wall_us: n,
            cpu: Some((n * 1000, n * 2000)),
            allocations: Snapshot {
                allocations: n,
                requested_bytes: n * 16,
                ..Snapshot::default()
            },
        }
    }
    #[test]
    fn completion_windows_exclude_warmup_and_do_not_overlap() {
        let mut m = Metrics::default();
        for n in 1..=288 {
            m.completed = n;
            m.completion_pending = true;
            m.end_loop_at(sample(n - 1), sample(n));
        }
        assert_eq!(m.main_wall_us.len(), 256);
        assert_eq!(m.main_cpu_ns.percentile(99), Some(1000));
        assert_eq!(m.process_cpu_ns.percentile(99), Some(2000));
        assert_eq!(m.allocations.allocations, 256);
        assert_eq!(m.allocations.requested_bytes, 4096);
        m.reset_windows();
        m.completed = 289;
        m.completion_pending = true;
        m.end_loop_at(sample(288), sample(289));
        assert_eq!(m.main_wall_us.len(), 1);
        assert_eq!(m.allocations.allocations, 1);
    }
    #[test]
    fn failed_presentation_cannot_supply_the_next_frames_path_or_copy_counts() {
        let mut m = Metrics::default();
        m.pending_cpu_bytes = 4096;
        m.pending_worker = Some(WorkerSample {
            wall_us: 20,
            cpu_ns: Some(10),
            submission_us: 5,
            interval_ns: None,
            readback_bytes: 4096,
        });
        m.cancel_pending();
        m.completed = 32;
        let frame = crate::presentation::PendingFrame {
            submission: bexos_graphics_runtime::presentation::Submission { deadline_us: 0 },
            scanout_buffer: 42,
            frame_time_us: 90,
            timer_deadline_us: 0,
            frozen: false,
            resumed: false,
            sequences: [(0, 0); 16],
            count: 0,
        };
        m.complete(100, &frame);
        assert_eq!(m.direct_frames, 1);
        assert_eq!(m.gpu_frames, 0);
        assert_eq!(m.cpu_output_bytes, 0);
        assert_eq!(m.worker_cpu_ns.len(), 0);
        assert_eq!(m.processing_us.percentile(99), Some(10));
    }

    #[test]
    fn entire_loop_and_worker_interval_are_distinct() {
        let mut m = Metrics::default();
        m.completed = 32;
        m.completion_pending = true;
        m.end_loop_at(sample(0), sample(1));
        m.end_loop_at(sample(1), sample(2));
        m.completed = 33;
        m.completion_pending = true;
        // Main thread slept between loop passes; process CPU includes its worker.
        m.end_loop_at(sample(4), sample(5));
        assert_eq!(m.main_wall_us.percentile(99), Some(2));
        assert_eq!(m.process_cpu_ns.percentile(99), Some(8000));
        assert_eq!(m.allocations.allocations, 4);
    }
}
