"""Run only Bazel-built optimized workloads; report machine and raw evidence."""
import argparse
import datetime
import json
import os
from pathlib import Path
import platform
import subprocess
from telemetry import read as read_guest_telemetry

parser = argparse.ArgumentParser()
parser.add_argument("runfiles", type=Path)
parser.add_argument("--output", type=Path, required=True)
parser.add_argument("--guest-log", type=Path, action="append", default=[],
                    help="Include an existing guest test log; may be repeated")
args = parser.parse_args()
machine = {"os": platform.platform(), "architecture": platform.machine(),
           "logical_cpus": os.cpu_count(), "processor": platform.processor()}
if platform.system() == "Darwin":
    for label, key in [("cpu", "machdep.cpu.brand_string"), ("memory_bytes", "hw.memsize")]:
        machine[label] = subprocess.run(["sysctl", "-n", key], check=True,
                                       text=True, capture_output=True, timeout=5).stdout.strip()
elif Path("/proc/cpuinfo").exists():
    machine["cpuinfo"] = Path("/proc/cpuinfo").read_text()
    machine["meminfo"] = Path("/proc/meminfo").read_text()
report = {"schema": 3, "started_utc": datetime.datetime.now(datetime.timezone.utc).isoformat(),
          "revision": subprocess.run(["git", "rev-parse", "HEAD"], cwd=os.environ.get("BUILD_WORKSPACE_DIRECTORY", "."), check=True, text=True, capture_output=True).stdout.strip(),
          "working_tree_dirty": bool(subprocess.run(["git", "status", "--porcelain"], cwd=os.environ.get("BUILD_WORKSPACE_DIRECTORY", "."), check=True, text=True, capture_output=True).stdout.strip()),
          "machine": machine, "build": "Bazel -c opt; enforced by benchmark binaries",
          "hardware_performance_verified": False, "results": [], "raw": {},
          "guest_observations": [read_guest_telemetry(path) for path in args.guest_log]}
for relative in ["lib/flatland/benchmark", "lib/flatland/blur_benchmark", "lib/flatland_cpu/benchmark"]:
    completed = subprocess.run([str(args.runfiles / relative)], check=True,
                               text=True, capture_output=True, timeout=300)
    report["raw"][relative] = completed.stdout
    report["results"].extend(json.loads(line) for line in completed.stdout.splitlines() if line.startswith("{"))
report["limitations"] = [
    "Host operation wall time includes descheduling; this is not full compositor CPU accounting.",
    "Scanout eligibility measures the scene decision only. Actual scanout and retirement use the QEMU fixtures.",
    "Output bytes count the affected destination region, not all memory reads or driver transfers.",
    "Guest logs can provide GPU queue intervals, input observations and cutovers; software Vulkan establishes functionality only.",
    "Guest CPU counters measure guest scheduled execution, not QEMU host-thread CPU usage.",
    "Guest CPU/allocation coverage is recorded per log and window; missing coverage is unavailable. Hardware input latency and physical display timing remain unestablished.",
]
args.output.parent.mkdir(parents=True, exist_ok=True)
args.output.write_text(json.dumps(report, indent=2) + "\n")
print(json.dumps(report, indent=2))
