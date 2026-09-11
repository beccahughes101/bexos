"""Bounded QMP workload controller shared by native and nested GPU fixtures."""
import json
import re
import time

WARMUP = 32
SAMPLES = 256
# One extra completion retires the last lease. This is only a harness budget.
WORKLOAD_SECONDS = (WARMUP + SAMPLES + 1) * 30 + 120


class Workloads:
    def __init__(self, request, workloads, gpu):
        self.request = request
        self.workloads = list(workloads)
        assert self.workloads and all(w in range(1, 6) for w in self.workloads)
        self.gpu = gpu
        self.index = 0
        self.started = False
        self.deadline = 0
        self.results = []
        self.completed_at = 0
        self.focus_offset = None

    def poll(self, log, pixel):
        if self.index == len(self.workloads):
            return True
        if isinstance(log, (bytes, bytearray)):
            log = log.decode(errors="replace")
        assert not re.search(r": panic:|libc: abort", log), "guest failed during sustained workload"
        workload = self.workloads[self.index]
        if not self.started:
            if self.focus_offset is None:
                self.focus_offset = len(log)
                self.deadline = time.monotonic() + WORKLOAD_SECONDS
                self.request('input-send-event', {'events': [
                    {'type':'abs','data':{'axis':'x','value':6000}},
                    {'type':'abs','data':{'axis':'y','value':6000}},
                    {'type':'btn','data':{'button':'left','down':True}},
                ]})
                self.request('input-send-event', {'events': [{'type':'btn','data':{'button':'left','down':False}}]})
                return False
            assert time.monotonic() < self.deadline - WORKLOAD_SECONDS + 30, 'surviving performance client did not receive focus'
            if 'input-fixture: view=0 kind=1 phase=1' not in log[self.focus_offset:]:
                return False
            self.request('input-send-event', {'events': [
                {'type': 'key', 'data': {'key': {'type': 'qcode', 'data': f'f{workload}'}, 'down': down}}
                for down in [True, False]]})
            self.started = True
            self.deadline = time.monotonic() + WORKLOAD_SECONDS
        assert time.monotonic() < self.deadline, f'scened workload {workload} timed out'
        if f'scened-workload: complete workload={workload} frames=288 leases=0' not in log:
            return False
        if not self.completed_at:
            self.completed_at = time.monotonic()
        assert time.monotonic() - self.completed_at < 30, f"workload {workload} final pixels missing: {None if pixel is None else (pixel(799,599),pixel(300,200),pixel(100,110),pixel(110,110))}"
        windows = []
        for line in log.splitlines():
            if line.startswith('scened-metrics: '):
                packet = json.loads(line[len('scened-metrics: '):])
                if packet.get('workload') == workload and packet.get('window_id') == 1:
                    windows.append(packet)
        assert windows, f'workload {workload} has no full compositor window'
        window = windows[-1]
        assert window['samples'] == window['cpu_samples'] == SAMPLES, window
        assert window['allocation_failures'] == window['failures'] == 0, window
        if workload == 5:
            assert window['direct_frames'] == SAMPLES and window['gpu_frames'] == 0, window
        elif self.gpu:
            assert window['gpu_frames'] == window['worker_cpu_samples'] == SAMPLES, window
            assert window['direct_frames'] == 0, window
        else:
            assert window['gpu_frames'] == window['direct_frames'] == 0, window
        if workload == 3:
            copied = window['gpu_readback_bytes' if self.gpu else 'cpu_output_bytes']
            assert 0 < copied < SAMPLES * 800 * 600 * 4 // 2, window
        # Uniform base with eight translucent overlays. Allow unorm rounding
        # differences; the sample is far from edges and rounded/blur boundaries.
        expected = (167, 83, 41)
        if pixel is None or any(abs(a-b) > 3 for a,b in zip(pixel(799,599),expected)):
            return False
        if workload in (2,4):
            color = pixel(300,200)
            if not all(abs(a-b) <= 4 for a,b in zip(color,(128,64,32))):
                return False
        if workload == 3:
            if any(abs(a-b)>3 for a,b in zip(pixel(100,110),expected)):
                return False
            if any(abs(a-b)>3 for a,b in zip(pixel(110,110),(147,73,36))):
                return False
        self.results.append(window)
        print('SCENED_WORKLOAD_VERIFIED ' + json.dumps({'workload':workload,'gpu':self.gpu,'window':window}),flush=True)
        self.index += 1
        self.started = False
        self.completed_at = 0
        self.focus_offset = None
        if self.index == len(self.workloads):
            print('SCENED_SUSTAINED_VERIFIED',flush=True)
            return True
        return False
