"""Exercise real input and preserve outstanding GPU/client work across replacement."""
from pathlib import Path
import os
import re
import subprocess
import time
from framebuffer import pixels
from scened_guest import Workloads
from input_hotplug import InputHotplug, InputDisconnect
import json


def verify(request, serial, client_path, work, children):
    logged = len(serial.read_bytes())
    def read_log():
        nonlocal logged
        data = serial.read_bytes()
        if len(data) > logged:
            print(data[logged:].decode(errors="replace"), end="", flush=True)
            logged = len(data)
        text = data.decode(errors="replace")
        assert re.search(r"panic:[^\n]*\n", text) is None, text
        return text
    def wait_for(marker, timeout=30):
        deadline = time.monotonic() + timeout
        while True:
            text = read_log()
            if marker in text:
                return text
            assert time.monotonic() < deadline, "missing fixture event: " + marker
            time.sleep(.05)
    def events(items):
        request("input-send-event", {"events": items})
    def key(name, down):
        return {"type": "key", "data": {"key": {"type": "qcode", "data": name}, "down": down}}
    def touch(phase, tracking, x=6000, y=10000, slot=2):
        values = [{"type": "mtt", "data": {"type": phase, "slot": slot, "tracking-id": tracking, "axis": "x", "value": 0}}]
        if tracking >= 0:
            values += [{"type": "mtt", "data": {"type": "data", "slot": slot, "tracking-id": tracking, "axis": axis, "value": value}}
                       for axis, value in [("x", x), ("y", y)]]
        events(values)
    events([key("f12", True), key("f12", False)])
    wait_for("input-fixture: accessibility controls verified")
    tablet = next(m for m in request("query-mice") if m["name"] == "QEMU Virtio Tablet")
    request("human-monitor-command", {"command-line": "mouse_set " + str(tablet["index"])})
    touch("begin", 17)
    touch("begin", 18, x=500, y=11000, slot=3)
    touch("update", 18, x=3000, y=11000, slot=3)
    events([{"type": "abs", "data": {"axis": "x", "value": 6000}},
            {"type": "abs", "data": {"axis": "y", "value": 10000}},
            {"type": "btn", "data": {"button": "left", "down": True}}])
    events([{"type": "btn", "data": {"button": "left", "down": False}}])
    wait_for("input-fixture: view=0 kind=1 phase=1")
    hotplug = InputHotplug(request)
    while not hotplug.poll(serial.read_bytes()):
        read_log()
        time.sleep(.05)
    print("input hotplug host-observed latency ms: " + json.dumps(hotplug.latencies_ms), flush=True)
    events([key("a", True)])
    wait_for("input-fixture: acquire held across transplant")
    wait_for("input-fixture: shell gesture phase=2")
    probe_path = Path(work) / "transplants.log"
    with probe_path.open("w") as log:
        probe = subprocess.Popen(["/fixture/transplant_probe", str(client_path)],
                                 stdout=log, stderr=subprocess.STDOUT,
                                 env=dict(os.environ, BOOT_UI_INPUT="1"))
        children.append(probe)
        # This encloses five signed archive preparations and two rejected
        # candidates under nested TCG. Individual transplant/cutover deadlines
        # remain enforced by the probe and guest; package staging is additional.
        deadline = time.monotonic() + 480
        probe_logged = 0
        def read_probe_log():
            nonlocal probe_logged
            # Use a separate open description: seeking the child's inherited
            # descriptor would change its output offset and corrupt evidence.
            result = probe_path.read_text(errors="replace")
            if len(result) > probe_logged:
                print(result[probe_logged:], end="", flush=True)
                probe_logged = len(result)
            return result
        try:
            while probe.poll() is None:
                read_log()
                read_probe_log()
                assert time.monotonic() < deadline, "nested transplant verifier exceeded 480 seconds"
                time.sleep(.1)
        finally:
            result = read_probe_log()
        assert probe.returncode == 0 and "graphics probe: transplants verified" in result, result
        assert "migration: target record adoption failed key=0 error=UnsupportedVersion" in read_log()
    touch("end", -1)
    touch("end", -1, slot=3)
    events([key("a", False)])
    wait_for("input-fixture: Venus context/mapping/fence survived transplant")
    wait_for("input-fixture: retired buffer lease after transplant")
    wait_for("input-fixture: shell gesture phase=3")
    events([{"type": "abs", "data": {"axis": "x", "value": 26000}},
            {"type": "btn", "data": {"button": "left", "down": True}}])
    events([{"type": "btn", "data": {"button": "left", "down": False}}])
    wait_for("input-fixture: view=1 kind=1 phase=1")
    events([key("b", True), key("b", False)])
    text = wait_for("input-fixture: view=1 kind=2 phase=0 code=48 state=0")
    assert "input-fixture: view=1 kind=2 phase=0 code=30 state=1" not in text
    assert "input-fixture: view=0 kind=2 phase=0 code=48 state=1" not in text
    events([key("f10", True), key("f10", False)])
    wait_for("scened: imported buffer scanout completed")
    wait_for("input-fixture: fullscreen opaque presentation complete")
    def wait_pixels(expected):
        deadline = time.monotonic() + 30
        while True:
            sample = pixels(Path(work) / "Xvfb_screen0")
            if all(sample(x, y) == color for x, y, color in expected):
                return
            read_log()
            assert time.monotonic() < deadline, "scanout/composition transition pixels missing"
            time.sleep(.05)
    wait_pixels([(0,0,(167,83,41)), (400,300,(167,83,41)), (799,599,(167,83,41))])
    events([key("f10", True), key("f10", False)])
    wait_for("input-fixture: composition restored; scanout lease retired")
    wait_pixels([(50,80,(30,60,150)), (700,80,(160,80,30))])
    wait_for("scened: Vello/Venus composed frame submitted")
    assert "input-fixture: CPU/Vulkan scene and backdrop ordering verified" in read_log()
    assert "input-fixture: GPU worker partial repair, failure and restart verified" in read_log()
    assert "input-fixture: CPU/Vulkan multilingual glyphs verified" in read_log()
    disconnect = InputDisconnect(request)
    while not disconnect.poll(serial.read_bytes()):
        read_log()
        time.sleep(.05)
    wait_pixels([(50,80,(30,60,150)), (700,80,(28,18,14))])
    print("BEXOS_VENUS_INPUT_TRANSPLANT_VERIFIED", flush=True)
    match = re.search(r"(?:^| )scened_workload=([1-6])(?: |$)", Path("/proc/cmdline").read_text().strip())
    if match:
        workloads = Workloads(request, range(1,6) if match.group(1) == "6" else [int(match.group(1))], True)
        while not workloads.poll(read_log(), pixels(Path(work) / "Xvfb_screen0")):
            time.sleep(.05)
