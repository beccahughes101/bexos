"""Verify the fixture host before a BexOS Vulkan workload can be accepted."""
from pathlib import Path
import json
import os
import re
import socket
import subprocess
import struct
import tempfile
import time
from vulkan_probe import probe
from framebuffer import verify, diagnose
from window import place_scanout
from input_transplant import verify as verify_input_transplant
from input_hotplug import HOTPLUG_ROOT_PORT
from renderer_stack import capture as capture_renderer_stack


def run(command):
    result = subprocess.run(command, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                            text=True, timeout=90)
    print(result.stdout, flush=True)
    result.check_returncode()
    return result.stdout


def renderer_diagnostics(root, stacks=False):
    """Bounded snapshots of this fixture's renderer threads."""
    processes = {}
    for directory in Path('/proc').glob('[0-9]*'):
        try:
            fields = (directory / 'stat').read_text().rsplit(')', 1)[1].split()
            processes[int(directory.name)] = (int(fields[1]), fields)
        except (OSError, ValueError, IndexError):
            continue
    selected = {root}
    # Render-server workers may be reparented after their launcher exits.
    for pid in processes:
        try:
            if (Path('/proc') / str(pid) / 'comm').read_text().startswith('virgl'):
                selected.add(pid)
        except OSError:
            pass
    for _ in range(8):
        selected.update(pid for pid, (parent, _) in processes.items() if parent in selected)
    for pid in sorted(selected):
        if pid not in processes:
            continue
        parent, fields = processes[pid]
        threads = []
        for thread in list((Path('/proc') / str(pid) / 'task').glob('[0-9]*'))[:32]:
            try:
                stats = (thread / 'stat').read_text().rsplit(')', 1)[1].split()
                threads.append((int(thread.name), (thread / 'comm').read_text().strip(),
                                (thread / 'wchan').read_text().strip(), int(stats[11]) + int(stats[12])))
            except OSError:
                pass
        print('VENUS_RENDERER_DIAGNOSTIC', {'pid': pid, 'parent': parent,
              'cpu_ticks': int(fields[11]) + int(fields[12]), 'threads': threads}, flush=True)
        if stacks:
            for tid, name, wait, ticks in threads:
                if tid != pid and wait == '0' and ticks > 100 and 'TCG' not in name:
                    capture_renderer_stack(pid, tid)


icds = list(Path("/usr/share/vulkan/icd.d").glob("lvp_icd*.json"))
assert len(icds) == 1, icds
os.environ["VK_DRIVER_FILES"] = str(icds[0])
os.environ["RENDER_SERVER_EXEC_PATH"] = "/fixture/renderer_monitor.py"
version = run(["qemu-system-aarch64", "--version"])
assert "11.0.3" in version
summary = run(["vulkaninfo", "--summary"])
assert "PHYSICAL_DEVICE_TYPE_CPU" in summary and "llvmpipe" in summary
probe()
properties = run(["qemu-system-aarch64", "-device", "virtio-gpu-gl-pci,help"])
assert "venus" in properties and "hostmem" in properties and "blob" in properties
with tempfile.TemporaryDirectory(prefix="venus-") as work:
    children = []
    try:
        with open(work + "/xvfb.log", "w+") as xlog, open(work + "/qemu.log", "w+") as qlog:
            xserver = subprocess.Popen(["Xvfb", ":99", "-screen", "0", "800x600x24", "-fbdir", work, "-nolisten", "tcp", "-ac"], stdout=xlog, stderr=subprocess.STDOUT)
            children.append(xserver)
            for _ in range(100):
                if Path("/tmp/.X11-unix/X99").exists():
                    break
                assert xserver.poll() is None, "Xvfb exited"
                time.sleep(.02)
            # The initial SDL window is 640x480; centering it would leave an
            # (80,60) offset after the guest changes the scanout to 800x600.
            os.environ.update(DISPLAY=":99", EGL_PLATFORM="x11", SDL_VIDEODRIVER="x11", SDL_VIDEO_WINDOW_POS="0,0")
            qmp_path = work + "/qmp"
            guest = Path("/fixture/bootfs").exists()
            memory = "1024" if guest else "256"
            command = ["qemu-system-aarch64", "-machine", "virt,secure=off,virtualization=on,memory-backend=mem,iommu=smmuv3,highmem-ecam=off,highmem-mmio=off",
                "-accel", "tcg",
                "-cpu", "cortex-a53", "-smp", "4", "-m", memory, "-object", "memory-backend-memfd,id=mem,size=" + memory + "M,share=on",
                "-nodefaults", "-device", "virtio-gpu-gl-pci,id=gpu0,blob=on,venus=on,hostmem=256M,xres=800,yres=600",
                "-display", "sdl,gl=on", "-S", "-qmp", "unix:" + qmp_path + ",server=on,wait=off"]
            serial = Path(work) / "serial.log"
            serial_path = Path(work) / "serial"
            if guest:
                command += ["-trace", "virtio_gpu_cmd_res_map_blob",
                            "-trace", "virtio_gpu_cmd_res_unmap_blob"]
                handoff = bytearray(Path("/fixture/handoff").read_bytes())
                words = struct.unpack("<" + "Q" * (len(handoff) // 8), handoff)
                handoff[72:80] = struct.pack("<Q", 1)
                handoff[80:112] = os.urandom(32)
                Path(work + "/handoff").write_bytes(handoff)
                command += ["-kernel", "/fixture/kernel", "-chardev", "socket,id=debug,path=" + str(serial_path) + ",server=on,wait=off,logfile=" + str(serial), "-serial", "chardev:debug",
                    "-drive", "if=none,id=nvme0,file=/dev/vda,format=raw,snapshot=on", "-device", "nvme,drive=nvme0,serial=bexos-nvme0",
                    "-netdev", "user,id=net0", "-device", "virtio-net-pci,disable-legacy=on,netdev=net0,mac=52:54:00:12:34:56"]
                for name in ("keyboard", "mouse", "tablet", "multitouch"):
                    command += ["-device", "virtio-" + name + "-pci"]
                command += ["-device", HOTPLUG_ROOT_PORT]
                for path, offset in [("/fixture/bootfs", words[4]), (work + "/handoff", 0x40100000), ("/fixture/evidence", words[14])]:
                    command += ["-device", f"loader,file={path},addr={offset:#x},force-raw=on"]
            qemu = subprocess.Popen(command,
                stdout=qlog, stderr=subprocess.STDOUT)
            children.append(qemu)
            with socket.socket(socket.AF_UNIX) as client:
                client.settimeout(10)
                for _ in range(500):
                    if qemu.poll() is not None:
                        qlog.seek(0)
                        raise AssertionError(qlog.read())
                    try:
                        client.connect(qmp_path)
                        break
                    except (FileNotFoundError, ConnectionRefusedError):
                        time.sleep(.02)
                stream = client.makefile("rwb", buffering=0)
                assert "QMP" in json.loads(stream.readline())
                def request(execute, arguments=None):
                    stream.write(json.dumps({"execute":execute,"arguments":arguments or {}}).encode()+b"\n")
                    while True:
                        result=json.loads(stream.readline())
                        assert "error" not in result,result
                        if "return" in result:return result["return"]
                request("qmp_capabilities")
                assert request("qom-get",{"path":"/machine/peripheral/gpu0","property":"venus"}) is True
                if guest:
                    request("cont")
                    deadline = time.monotonic() + 420
                    logged = 0
                    diagnosed_pipelines = False
                    captured_renderer = False
                    while True:
                        log = serial.read_text(errors="replace") if serial.exists() else ""
                        if len(log) > logged:
                            print(log[logged:], end="", flush=True)
                            logged = len(log)
                        if "input-fixture: GPU capabilities verified" in log and "input-fixture: accessibility magnification active" in log:
                            break
                        if not diagnosed_pipelines and "input-fixture: Vello pipelines created" in log:
                            renderer_diagnostics(qemu.pid)
                            diagnosed_pipelines = True
                        if not captured_renderer and "stuck in ring seqno wait with iter at 4096" in log:
                            renderer_diagnostics(qemu.pid, stacks=True)
                            qlog.flush()
                            print(Path(work + "/qemu.log").read_text(), flush=True)
                            captured_renderer = True
                        if "VENUS_RENDERER_FATAL" in Path(work + "/qemu.log").read_text() or qemu.poll() is not None or time.monotonic() >= deadline or re.search(r"panic:[^\n]*\n", log) or "appd: boot failed:" in log or "virtio-gpu: command failed" in log or "kernel: process exit package=bexos.testing.input_fixture" in log:
                            renderer_diagnostics(qemu.pid)
                            qlog.seek(0)
                            raise AssertionError(qlog.read() + "\n" + log)
                        time.sleep(.1)
                    assert "venus_offered=true" in log, log
                    assert "input-fixture: Venus context/shared-memory/fence verified" in log, log
                    assert "input-fixture: guest Venus Vulkan version=" in log, log
                    place_scanout()
                    print(log, flush=True)
                    deadline = time.monotonic() + 30
                    while True:
                        valid, samples = verify(Path(work) / "Xvfb_screen0")
                        if valid:
                            break
                        if qemu.poll() is not None or time.monotonic() >= deadline:
                            qlog.seek(0)
                            raise AssertionError("guest scanout samples: " + repr(samples) + "\n" + repr(diagnose(Path(work) / "Xvfb_screen0")) + "\n" + qlog.read()
                                                 + "\n" + serial.read_text(errors="replace"))
                        time.sleep(.1)
                    print("BEXOS_VENUS_DEVICE_CPU_SCANOUT_VERIFIED", flush=True)
                    verify_input_transplant(request, serial, serial_path, work, children)
                request("quit")
            assert qemu.wait(timeout=5)==0
            print("VENUS_LINUX_DEVICE_CREATED", flush=True)
    finally:
        for child in reversed(children):
            if child.poll() is None:
                child.terminate()
                try:child.wait(timeout=3)
                except subprocess.TimeoutExpired:child.kill();child.wait()
print("VENUS_LINUX_HOST_VERIFIED", flush=True)
