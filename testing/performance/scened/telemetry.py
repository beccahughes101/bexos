"""Collect measured guest evidence without treating functional runs as acceptance."""
import json
import math
from pathlib import Path
import re


MARKERS = {
    "scened-metrics: ": "compositor_windows",
    "input-fixture: worker timing ": "gpu_worker_observations",
    "input hotplug host-observed latency ms: ": "input_latency_observations_ms",
}


def _constant(value):
    raise ValueError("nonfinite telemetry value: " + value)


def _measurement(packet):
    if not isinstance(packet, dict) or not packet:
        raise ValueError("telemetry must be a nonempty object")
    for key, value in packet.items():
        if not isinstance(key, str):
            raise ValueError("telemetry names must be strings")
        if isinstance(value, bool):
            if key not in ("hardware_performance_verified", "hardware_vsync"):
                raise ValueError("boolean supplied as a measurement")
        elif value is not None:
            if (not isinstance(value, (int, float)) or value < 0 or value > 2**64 - 1
                    or not math.isfinite(value)):
                raise ValueError("invalid measurement: " + key)
    # Acceptance is not inferred from a log assertion or from software timings.
    return packet


def collect(text):
    lines = text.splitlines()
    markers = {line.strip() for line in lines}
    result = {name: [] for name in MARKERS.values()}
    result.update({
        "hardware_performance_verified": False,
        "scope": "functional guest integration; observations are not hardware acceptance",
        "duplicate_log_observations_removed": True,
        "cutovers": [],
        "compositor_gpu_frame_observed": "scened: Vello/Venus composed frame submitted" in markers,
        "integration_passed": ("SCENED_NATIVE_FIXTURE_VERIFIED" in markers or
                               ("BEXOS_VENUS_INPUT_TRANSPLANT_VERIFIED" in markers
                                and "VENUS_LINUX_FIXTURE_EXIT=0" in markers)),
        "guest_cpu_accounting_available": False,
        "guest_allocation_counts_available": False,
    })
    # The fixture reprints serial sections for reference checks and failures.
    # Keep unique observations, not a statistical sample count of printed lines.
    seen = {name: set() for name in MARKERS.values()}
    cutovers = set()
    for line in lines:
        line = line.strip()
        for marker, name in MARKERS.items():
            if not line.startswith(marker):
                continue
            payload = line[len(marker):].lstrip()
            packet, end = json.JSONDecoder(parse_constant=_constant).raw_decode(payload)
            if payload[end:].strip():
                raise ValueError("trailing data after telemetry object")
            packet = _measurement(packet)
            signature = json.dumps(packet, sort_keys=True)
            if signature not in seen[name]:
                seen[name].add(signature)
                result[name].append(packet)
        match = re.search(r"service-transplant: committed generation=(\d+) cutover_ms=(\d+)", line)
        if match:
            pair = tuple(map(int, match.groups()))
            if pair not in cutovers:
                cutovers.add(pair)
                result["cutovers"].append({"generation": pair[0], "wall_ms": pair[1]})
    windows = result["compositor_windows"]
    identified = {}
    for packet in windows:
        if packet.get("schema", 0) < 3:
            continue
        required = ["epoch_us", "window_id", "samples", "window", "cpu_samples", "allocations", "allocation_bytes", "allocation_failures", "workload"]
        if any(type(packet.get(key)) is not int for key in required):
            raise ValueError("incomplete identified compositor window")
        if packet["samples"] != 256 or packet["window"] != 256 or not 0 <= packet["cpu_samples"] <= 256:
            raise ValueError("invalid compositor sample counts")
        if packet["epoch_us"] == 0 or packet["window_id"] == 0 or not 0 <= packet["workload"] <= 5:
            raise ValueError("invalid compositor window identity")
        for count in ("worker_cpu_samples", "gpu_timestamp_samples", "gpu_frames", "direct_frames", "missed_timer_deadlines"):
            if count in packet and (type(packet[count]) is not int or not 0 <= packet[count] <= 256):
                raise ValueError("invalid compositor sample count: " + count)
        if packet.get("gpu_frames", 0) + packet.get("direct_frames", 0) > 256:
            raise ValueError("overlapping compositor rendering paths")
        key = packet["epoch_us"], packet["window_id"]
        if key in identified and packet != identified[key]:
            raise ValueError("conflicting compositor window identity")
        identified[key] = packet
    result["guest_cpu_accounting_available"] = any(p["cpu_samples"] == 256 and p.get("process_cpu_p99_ns") is not None for p in identified.values())
    result["guest_allocation_counts_available"] = bool(identified)
    result["sustained_passed"] = bool(identified) and all(p["cpu_samples"] == 256 for p in identified.values()) and "SCENED_SUSTAINED_VERIFIED" in markers and (
        "VENUS_LINUX_FIXTURE_EXIT=0" in markers or "SCENED_NATIVE_FIXTURE_VERIFIED" in markers)
    result["measured_workloads"] = sorted({p["workload"] for p in identified.values() if p["workload"]})
    result["cpu_accounting_scope"] = "scheduled process execution including worker threads and kernel calls; excludes blocked time" if result["guest_cpu_accounting_available"] else None
    result["allocation_scope"] = "successful logical Rust/C allocation and reallocation requests; excludes non-allocator VMOs" if identified else None
    return result


def read(path):
    path = Path(path)
    if path.stat().st_size > 16 * 1024 * 1024:
        raise ValueError("guest telemetry log exceeds 16 MiB")
    result = collect(path.read_text(errors="replace"))
    result["source"] = str(path.resolve())
    return result
