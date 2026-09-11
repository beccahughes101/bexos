"""Verify QEMU root-port hot-add and removal on both fixture machine types.

The full guest test configures bridge bus numbers and checks PCI discovery;
this paused-machine check validates the QMP operation and device lifetime.
"""
import json
from pathlib import Path
import socket
import subprocess
import tempfile
import time
from input_hotplug import HOTPLUG_ROOT_PORT


def verify(architecture, machine):
    with tempfile.TemporaryDirectory(prefix="input-pci-") as directory:
        path = str(Path(directory) / "qmp")
        process = subprocess.Popen([
            "qemu-system-" + architecture, "-machine", machine, "-accel", "tcg",
            "-m", "128", "-S", "-nodefaults", "-display", "none", "-nic", "none",
            "-device", HOTPLUG_ROOT_PORT, "-qmp", "unix:" + path + ",server=on,wait=off",
        ], stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
        try:
            with socket.socket(socket.AF_UNIX) as connection:
                connection.settimeout(2)
                deadline = time.monotonic() + 10
                while True:
                    assert process.poll() is None, process.stderr.read().decode()
                    try:
                        connection.connect(path)
                        break
                    except (FileNotFoundError, ConnectionRefusedError):
                        assert time.monotonic() < deadline, "QMP startup timeout"
                        time.sleep(.02)
                stream = connection.makefile("rwb", buffering=0)
                assert "QMP" in json.loads(stream.readline())
                def request(command, arguments=None):
                    stream.write(json.dumps({"execute": command, "arguments": arguments or {}}).encode() + b"\n")
                    while True:
                        result = json.loads(stream.readline())
                        assert "error" not in result, result
                        if "return" in result:
                            return result["return"]
                request("qmp_capabilities")
                def present():
                    def contains(devices):
                        return any(device.get("qdev_id") == "hotkey" or
                                   contains(device.get("pci_bridge", {}).get("devices", []))
                                   for device in devices)
                    return any(contains(bus["devices"]) for bus in request("query-pci"))
                assert not present(), "empty root port contains keyboard"
                request("device_add", {"driver": "virtio-keyboard-pci", "id": "hotkey", "bus": "input-slot"})
                assert request("qom-get", {"path": "/machine/peripheral/hotkey", "property": "realized"}) is True
                request("qom-set", {"path": "/machine/peripheral/hotkey", "property": "realized", "value": False})
                assert request("qom-get", {"path": "/machine/peripheral/hotkey", "property": "realized"}) is False
                stream.close()
        finally:
            if process.poll() is None:
                process.terminate()
            try:
                process.wait(timeout=3)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait()
            diagnostic = process.stderr.read().decode(errors="replace")
            if diagnostic:
                print(diagnostic, flush=True)
            process.stderr.close()
    print(architecture + ": QEMU root-port keyboard hot-add and removal verified")


verify("aarch64", "virt")
verify("x86_64", "q35")
