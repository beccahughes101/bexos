"""Boot the real graphical product; retain screenshots and always stop QEMU."""
import hashlib
from contextlib import ExitStack
import shutil
import json
import os
import re
from pathlib import Path
import selectors
import socket
import struct
import subprocess
import sys
import signal
import tempfile
import time
sys.path.insert(0, str(Path(os.environ.get("RUNFILES_DIR", ".")) / "_main/testing/performance/scened"))
from guest import Workloads
from input_hotplug import InputHotplug, InputDisconnect, HOTPLUG_ROOT_PORT

# Bazel timeout/cancellation must unwind the same child cleanup as failures.
signal.signal(signal.SIGTERM, lambda *_: sys.exit(143))

arch, kernel, bootfs, handoff, evidence, disk = sys.argv[1:]
performance = bool(os.environ.get("SCENED_SUSTAINED"))
output_dir = Path(os.environ.get('TEST_UNDECLARED_OUTPUTS_DIR', '/tmp'))
output_dir.mkdir(parents=True, exist_ok=True)
def stop(child):
    if child.poll() is None:
        child.terminate()
        try:
            child.wait(timeout=3)
        except subprocess.TimeoutExpired:
            child.kill()
            child.wait()

kernel, bootfs, handoff, evidence, disk = map(lambda p: str(Path(p).resolve()), [kernel, bootfs, handoff, evidence, disk])
with tempfile.TemporaryDirectory(prefix='bex-ui-', dir='/tmp') as scratch, ExitStack() as cleanup:
    qmp_path = scratch + '/qmp'
    debug_path = scratch + '/debug'
    with open(handoff, 'rb') as f:
        words = struct.unpack('<' + 'Q' * (os.path.getsize(handoff) // 8), f.read())
    aarch64 = arch == 'aarch64'
    no_gpu = bool(os.environ.get('BOOT_UI_NO_GPU'))
    test_input = bool(os.environ.get('BOOT_UI_INPUT'))
    dioxus_smoke = bool(os.environ.get('BOOT_UI_DIOXUS'))
    width = int(os.environ.get('BOOT_UI_WIDTH', '800'))
    height = int(os.environ.get('BOOT_UI_HEIGHT', '600'))
    assert 640 <= width <= 4096 and 480 <= height <= 4096
    assert not test_input or (width, height) == (800, 600), 'Input reference coordinates require 800x600'
    gpu_device = f'virtio-gpu-pci,xres={width},yres={height}'
    accessibility_visual = False
    input_clicked = False
    input_sent = False
    input_released = False
    input_right_clicked = False
    scanout_phase = 0
    scanout_visual = False
    composition_visual = False
    firmware = os.environ.get('BOOT_UI_FIRMWARE')
    if firmware:
        assert aarch64
        root = Path(firmware).resolve().parent
        for name in ['bl1.bin','bl2.bin','bl31.bin','bl33.bin','tb_fw.crt','trusted_key.crt','soc_fw_key.crt','tos_fw_key.crt','nt_fw_key.crt','soc_fw_content.crt','tos_fw_content.crt','nt_fw_content.crt']:
            shutil.copyfile(root / name, Path(scratch) / name)
        shutil.copyfile(root / 'lk.bin', Path(scratch) / 'bl32.bin')
        shutil.copyfile(root / 'RPMB_DATA', Path(scratch) / 'RPMB_DATA')
        rpmb_path = scratch + '/rpmb.sock'
        rpmb = subprocess.Popen([str(root / 'rpmb_dev'), '--dev', scratch + '/RPMB_DATA', '--sock', rpmb_path], stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
        cleanup.callback(stop, rpmb)
        for _ in range(200):
            if Path(rpmb_path).exists():
                break
            if rpmb.poll() is not None:
                raise AssertionError(rpmb.stderr.read().decode(errors='replace'))
            time.sleep(.025)
        assert Path(rpmb_path).exists(), 'RPMB proxy did not start'
    seeded = bytearray(Path(handoff).read_bytes())
    seeded[72:80] = struct.pack('<Q',1)
    seeded[80:112] = os.urandom(32)
    handoff = scratch + '/handoff.bin'
    Path(handoff).write_bytes(seeded)
    machine = 'virt,secure=off,virtualization=on,iommu=smmuv3,highmem-ecam=off,highmem-mmio=off,accel=tcg' if aarch64 else 'q35,accel=tcg'
    command = [f'qemu-system-{arch}', '-machine', machine, '-cpu', 'cortex-a53' if aarch64 else 'max', '-smp', '4', '-m', '1024', '-display', 'none', '-vga', 'none', '-monitor', 'none', '-serial', 'stdio', '-no-reboot', '-kernel', kernel,
               '-qmp', f'unix:{qmp_path},server=on,wait=off', '-device', gpu_device,
               '-drive', f'if=none,id=nvme0,file={disk},format=raw,snapshot=on', '-device', 'nvme,drive=nvme0,serial=bexos-nvme0',
               '-netdev', 'user,id=net0', '-device', 'virtio-net-pci,disable-legacy=on,netdev=net0,mac=52:54:00:12:34:56']
    if test_input:
        command += ['-device','virtio-keyboard-pci,id=keyboard0','-device','virtio-mouse-pci,id=mouse0','-device','virtio-tablet-pci,id=tablet0','-device','virtio-multitouch-pci,id=touch0']
        command += ['-device', HOTPLUG_ROOT_PORT]
    if no_gpu:
        index = command.index(gpu_device)
        del command[index-1:index+1]
    if not aarch64:
        command += ['-device', 'virtio-serial-pci,disable-legacy=on', '-chardev', f'socket,id=console,path={debug_path},server=on,wait=off', '-device', 'virtserialport,chardev=console,nr=1,name=debug0']
    for path, address in [(bootfs, words[4]), (handoff, 0x40100000 if aarch64 else 0x01000000), (evidence, words[14])]:
        command += ['-device', f'loader,file={path},addr={address:#x},force-raw=on']
    if firmware:
        command[command.index('-machine')+1] = machine.replace('secure=off','secure=on')
        index = command.index('-kernel')
        del command[index:index+2]
        command += ['-bios',scratch+'/bl1.bin','-semihosting-config','enable=on,target=native',
                    '-device',f'loader,file={kernel},addr=0x40200000,force-raw=on',
                    '-device',f'loader,file={Path(os.environ["BOOT_UI_VBMETA"]).resolve()},addr=0x43e00000,force-raw=on',
                    # BL33 uses the reserved PCI UART before normal-world
                    # virtio-console takes ownership of the same RPMB helper.
                    '-chardev',f'socket,id=bootrpmb,path={rpmb_path}',
                    '-device','pci-serial,addr=7,chardev=bootrpmb',
                    '-device','virtio-serial-pci,disable-legacy=on',
                    '-device','virtserialport,chardev=rpmb0,name=rpmb0,nr=1',
                    '-chardev',f'socket,id=rpmb0,path={rpmb_path}']
    process = subprocess.Popen(command, cwd=scratch if firmware else None, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, stdin=subprocess.PIPE)
    cleanup.callback(stop, process)
    qmp = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    qmp.settimeout(2)
    debug = None
    log = bytearray()
    screenshots = []
    spinner_frames = set()
    progress_frames = set()
    desktop_visible = False
    probe = None
    peer = None
    listener = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    proxy_path = scratch + '/probe'
    listener.bind(proxy_path)
    listener.listen(1)
    listener.settimeout(2)
    probe_log = bytearray()
    probe_done = False
    validate_splash = bool(os.environ.get("BOOT_UI_VALIDATE_SPLASH"))
    expected_profiles = 1 if dioxus_smoke else 4 if validate_splash else 3

    try:
        for _ in range(200):
            try:
                qmp.connect(qmp_path)
                break
            except (FileNotFoundError, ConnectionRefusedError):
                if process.poll() is not None:
                    raise AssertionError(process.stdout.read().decode(errors='replace'))
                time.sleep(.02)
        stream = qmp.makefile('rwb', buffering=0)
        stream.readline()
        def request(name, arguments=None):
            stream.write(json.dumps({'execute': name, 'arguments': arguments or {}}).encode() + b'\n')
            while True:
                response = json.loads(stream.readline())
                if 'error' in response:
                    raise AssertionError(response)
                if 'return' in response:
                    return response['return']
        request('qmp_capabilities')
        workloads = Workloads(request, range(1,6), False) if performance else None
        current_pixel = None
        hotplug = InputHotplug(request)
        hotplug_done = False
        disconnect = InputDisconnect(request)
        disconnect_done = False
        disconnect_visual = False
        def select_pointer(name):
            device = next(m for m in request('query-mice') if m['name'] == name)
            request('human-monitor-command', {'command-line': f"mouse_set {device['index']}"})
            assert next(m for m in request('query-mice') if m['name'] == name)['current']
        def touch(phase, tracking, x=6000, y=10000, slot=2):
            events = [{'type':'mtt','data':{'type':phase,'slot':slot,'tracking-id':tracking,'axis':'x','value':0}}]
            if tracking >= 0:
                events += [{'type':'mtt','data':{'type':'data','slot':slot,'tracking-id':tracking,'axis':axis,'value':value}} for axis,value in [('x',x),('y',y)]]
            request('input-send-event', {'events':events})
        if not aarch64:
            debug = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
            debug.connect(debug_path)
        # Four active-input replacements include archive verification, UART
        # diagnostics, and the native text/GPU renderer stack under TCG. Keep
        # an overall bound without confusing this functional boot budget with
        # the guest's unchanged cutover deadlines.
        boot_timeout = int(os.environ.get("BOOT_UI_TIMEOUT_SECONDS", "900"))
        assert 60 <= boot_timeout <= 3600, "invalid graphical boot timeout"
        deadline = time.monotonic() + boot_timeout
        next_capture = 0
        done_at = None
        with selectors.DefaultSelector() as poll:
            poll.register(process.stdout, selectors.EVENT_READ)
            if debug is not None:
                poll.register(debug, selectors.EVENT_READ)
            while True:
                for key, _ in poll.select(.025):
                    if key.fileobj is peer:
                        chunk = peer.recv(65536)
                        if not chunk:
                            poll.unregister(peer)
                            continue
                        if debug is not None:
                            debug.sendall(chunk)
                        else:
                            process.stdin.write(chunk)
                            process.stdin.flush()
                        continue
                    if probe is not None and key.fileobj is probe.stdout:
                        chunk = probe.stdout.read1(65536)
                        if not chunk:
                            poll.unregister(probe.stdout)
                        probe_log.extend(chunk)
                        print(chunk.decode(errors='replace'), end='', flush=True)
                        continue
                    chunk = key.fileobj.recv(65536) if key.fileobj is debug else key.fileobj.read1(65536)
                    log.extend(chunk)
                    if peer is not None and ((debug is None and key.fileobj is process.stdout) or key.fileobj is debug):
                        try:
                            peer.sendall(chunk)
                        except BrokenPipeError:
                            pass
                    print(chunk.decode(errors='replace'), end='', flush=True)
                now = time.monotonic()
                if no_gpu and b'splashd: stage=5 percent=100' in log:
                    assert b'appd: pivot complete' in log
                    print(f'Validated graphical product boot without GPU on {arch}')
                    break
                trigger = b'appd: graphical lifecycle test awaiting replacements' if validate_splash else b'scened: ready background presented'
                if trigger in log and done_at is None:
                    done_at = now + .5
                    print('boot-ui-harness: probe trigger observed', flush=True)
                if (not test_input and done_at is not None and now >= done_at
                        and probe is None):
                    probe_path = os.environ['BOOT_UI_DIOXUS_PROBE'] if dioxus_smoke else os.environ['BOOT_UI_PROBE']
                    print(f'boot-ui-harness: starting {Path(probe_path).name}', flush=True)
                    probe = subprocess.Popen([probe_path, proxy_path], stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
                    peer, _ = listener.accept()
                    print('boot-ui-harness: probe connected', flush=True)
                    poll.register(peer, selectors.EVENT_READ)
                    poll.register(probe.stdout, selectors.EVENT_READ)
                capture_allowed = test_input or validate_splash or probe_done or dioxus_smoke
                if now >= next_capture and capture_allowed and b'splashd: first frame presented' in log:
                    path = output_dir / f'frame-{len(screenshots):04}.ppm'
                    request('screendump', {'filename': str(path.resolve())})
                    pixels = path.read_bytes()
                    digest = hashlib.sha256(pixels).hexdigest()
                    if screenshots and screenshots[-1][1] == digest:
                        path.unlink()
                    else:
                        screenshots.append((now, digest, path))
                    magic, dimensions, maximum, rgb = pixels.split(b'\n', 3)
                    assert (magic, dimensions, maximum) == (b'P6', f'{width} {height}'.encode(), b'255')
                    assert len(rgb) == width * height * 3
                    current_pixel = lambda x,y: tuple(rgb[(y*width+x)*3:(y*width+x)*3+3])
                    if test_input and not accessibility_visual and b'input-fixture: accessibility magnification active' in log:
                        def pixel(x, y):
                            i = (y * width + x) * 3
                            return rgb[i:i+3]
                        # The scaled left/right boundary is x=620. The focus ring
                        # is red before the grayscale pass, with luma 54.
                        if (pixel(120, 100) == bytes((54, 54, 54))
                                and pixel(500, 80) == bytes((60, 60, 60))
                                and pixel(700, 80) == bytes((93, 93, 93))):
                            accessibility_visual = True
                            request('input-send-event', {'events':[
                                {'type':'key','data':{'key':{'type':'qcode','data':'f12'},'down':True}},
                                {'type':'key','data':{'key':{'type':'qcode','data':'f12'},'down':False}}]})
                    if test_input and scanout_phase == 1 and b'scened: imported buffer scanout completed' in log and b'input-fixture: fullscreen opaque presentation complete' in log:
                        expected = bytes((167, 83, 41))
                        if rgb == expected * (width * height):
                            scanout_visual = True
                            scanout_phase = 2
                            request('input-send-event', {'events':[
                                {'type':'key','data':{'key':{'type':'qcode','data':'f10'},'down':True}},
                                {'type':'key','data':{'key':{'type':'qcode','data':'f10'},'down':False}}]})
                    if test_input and scanout_phase == 2 and b'input-fixture: composition restored; scanout lease retired' in log:
                        left = (80 * width + 50) * 3
                        right = (80 * width + 700) * 3
                        composition_visual |= rgb[left:left+3] == bytes((30, 60, 150)) and rgb[right:right+3] == bytes((160, 80, 30))
                        if disconnect_done:
                            disconnect_visual |= rgb[left:left+3] == bytes((30, 60, 150)) and rgb[right:right+3] == bytes((28, 18, 14))
                    if b'scened: styled desktop text presented' in log and not test_input:
                        # The default prototxt's text color, sampled only in
                        # the central desktop text bounds after splash takeover.
                        matches = sum(rgb[i:i+3] == bytes((214, 228, 255))
                                      for y in range(height//2-80, height//2+90)
                                      for i in range((y*width+width//2-250)*3, (y*width+width//2+250)*3, 3))
                        desktop_visible |= matches > 100
                    if b'scened: splash frame presented; takeover acknowledged' not in log:
                        def region(x0, y0, x1, y1):
                            return b''.join(rgb[(y*width+x0)*3:(y*width+x1)*3] for y in range(y0,y1))
                        scale = min(width / 640, height / 480)
                        cx, cy = width / 2, height / 2
                        spinner_frames.add(hashlib.sha256(region(int(cx-20*scale), int(cy-4*scale), int(cx+20*scale), int(cy+32*scale))).hexdigest())
                        progress_frames.add(hashlib.sha256(region(int(cx-80*scale), int(cy+53*scale), int(cx+80*scale), int(cy+56*scale)+1)).hexdigest())
                    next_capture = now + (1 if b'scened: ready background presented' in log else .25)
                if test_input and done_at is not None and not input_clicked and b'input-fixture: accessibility controls verified' in log:
                    select_pointer('QEMU Virtio Mouse')
                    request('input-send-event', {'events':[{'type':'rel','data':{'axis':'x','value':50}},{'type':'rel','data':{'axis':'y','value':30}}]})
                    select_pointer('QEMU Virtio Tablet')
                    touch('begin', 17)
                    touch('begin', 18, x=500, y=11000, slot=3)
                    touch('update', 18, x=3000, y=11000, slot=3)
                    request('input-send-event', {'events':[{'type':'abs','data':{'axis':'x','value':6000}},{'type':'abs','data':{'axis':'y','value':10000}},{'type':'btn','data':{'button':'left','down':True}}]})
                    request('input-send-event', {'events':[{'type':'btn','data':{'button':'left','down':False}}]})
                    input_clicked=True
                if test_input and input_clicked and not input_sent and b'input-fixture: view=0 kind=1 phase=1' in log:
                    hotplug_done = hotplug.poll(log)
                    if hotplug_done:
                        request('input-send-event', {'events':[{'type':'key','data':{'key':{'type':'qcode','data':'a'},'down':True}}]})
                        input_sent=True
                        done_at=now+1
                if test_input and probe_done and not input_right_clicked:
                    touch('end', -1)
                    touch('end', -1, slot=3)
                    request('input-send-event', {'events':[{'type':'key','data':{'key':{'type':'qcode','data':'a'},'down':False}}]})
                    request('input-send-event', {'events':[{'type':'abs','data':{'axis':'x','value':26000}},{'type':'btn','data':{'button':'left','down':True}}]})
                    request('input-send-event', {'events':[{'type':'btn','data':{'button':'left','down':False}}]})
                    input_right_clicked=True
                if test_input and input_right_clicked and not input_released and b'input-fixture: view=1 kind=1 phase=1' in log:
                    request('input-send-event', {'events':[{'type':'key','data':{'key':{'type':'qcode','data':'b'},'down':True}},{'type':'key','data':{'key':{'type':'qcode','data':'b'},'down':False}}]})
                    input_released=True
                if test_input and scanout_phase == 0 and b'input-fixture: view=1 kind=2 phase=0 code=48 state=0' in log and b'input-fixture: retired buffer lease after transplant' in log:
                    request('input-send-event', {'events':[
                        {'type':'key','data':{'key':{'type':'qcode','data':'f10'},'down':True}},
                        {'type':'key','data':{'key':{'type':'qcode','data':'f10'},'down':False}}]})
                    scanout_phase = 1
                if done_at is not None and now >= done_at and probe is None and (test_input and input_sent and b'input-fixture: acquire held across transplant' in log and b'input-fixture: shell gesture phase=2' in log):
                    probe = subprocess.Popen([os.environ['BOOT_UI_PROBE'], proxy_path], stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
                    peer, _ = listener.accept()
                    poll.register(peer, selectors.EVENT_READ)
                    poll.register(probe.stdout, selectors.EVENT_READ)
                if probe is not None and probe.poll() is not None and not probe_done:
                    probe_log.extend(probe.stdout.read())
                    assert probe.returncode == 0, probe_log.decode(errors='replace')
                    probe_done = True
                if dioxus_smoke and probe_done:
                    assert b'wasm_runner: native UI first frame submitted backend=' in log
                    break
                if test_input and composition_visual and hotplug_done and not disconnect_done:
                    disconnect_done = disconnect.poll(log)
                if (probe_done and (not test_input or b'input-fixture: view=1 kind=2 phase=0 code=48 state=0' in log) and b'scened: ready background presented' in log
                        and b'scened: post-transplant presentation complete' in log
                        and b'scened: styled desktop text presented' in log
                        and (not test_input or b'input-fixture: retired buffer lease after transplant' in log)
                        and (not test_input or b'input-fixture: shell gesture phase=3' in log)
                        and (not test_input or (scanout_visual and composition_visual and hotplug_done and disconnect_visual))
                        and (test_input or desktop_visible)
                        and log.count(b'appd: graphics deadline profile applied') >= expected_profiles
                        # Profile attachment can be logged just before splash
                        # cleanup in the same appd dispatch. Drain that final
                        # acknowledgment before stopping QEMU and asserting it.
                        and b'appd: splash completed; restart disabled' in log):
                    if workloads is not None and not workloads.poll(log, current_pixel):
                        deadline = max(deadline, workloads.deadline + 10)
                        continue
                    assert b'virtio-gpu: post-transplant presentation' in log
                    if validate_splash:
                        assert b'splashd: post-transplant animation presented' in log
                    break
                if now >= deadline or process.poll() is not None or b'bl33: FATAL:' in log or re.search(rb'appd: boot failed[^\n]*\n', log) or re.search(rb': panic:[^\n]*\n', log) or b'virtio-gpu: unavailable' in log or re.search(rb'scened: takeover failed[^\n]*\n', log) or re.search(rb'virtio-input: initialization failed[^\n]*\n', log):
                    raise AssertionError('Graphical boot did not complete:\n' + log.decode(errors='replace'))
        if no_gpu:
            sys.exit(0)
        if dioxus_smoke:
            combined = log + probe_log
            for marker in [f'virtio-gpu: ready {width}x{height}'.encode(), b'fontd: ready', b'scened: ready background presented', b'wasm_runner: composed component graph dependencies=1', b'wasm_runner: service component instantiated', b'wasm_runner: native UI first frame submitted backend=', b'dioxus-probe: Dioxus WASM app launched and rendered a native UI frame']:
                assert marker in combined, marker
            print(f'Validated Dioxus WASM native UI smoke on {arch}')
            sys.exit(0)
        if test_input:
            assert b'migration: target record adoption failed key=0 error=UnsupportedVersion' in log
            assert scanout_visual and composition_visual, 'Direct scanout/composition transition pixels missing'
            assert b'input-fixture: styled rounded backdrop staged' in log
            assert b'input-fixture: C mutex contention and once verified' in log
            assert b'input-fixture: C condition signal, broadcast and sleeping deadlines verified' in log
            assert log.count(b'input-fixture: GPU capabilities verified') >= 2
            assert accessibility_visual, 'Magnification, grayscale and focus-ring pixels were not observed'
            assert b'input-fixture: accessibility tokens survived transplant' in log
            assert b'input-fixture: shell gesture phase=3' in log
            assert re.search(rb'input-fixture: view=0 kind=1 phase=5[^\n]*id=4', log), 'Gesture takeover did not cancel the original recipient'
            assert not re.search(rb'input-fixture: view=\d kind=1 phase=3[^\n]*id=4', log), 'Claimed gesture release leaked to a view'
            assert b'input-fixture: view=0 kind=2 phase=0 code=30 state=1' in log
            assert b'input-fixture: view=0 kind=2 phase=0 code=30 state=0' in log
            assert b'input-fixture: view=1 kind=2 phase=0 code=30 state=1' not in log
            assert b'input-fixture: view=0 kind=2 phase=0 code=48 state=1' not in log
            assert b'virtio-input: queues and report stream adopted' in log
            assert re.search(rb'input-fixture: view=0 kind=1 phase=1[^\n]*id=3', log)
            assert re.search(rb'input-fixture: view=0 kind=1 phase=3[^\n]*id=3', log)
        for marker in [f'virtio-gpu: ready {width}x{height}'.encode(), b'fontd: ready', b'virtio-gpu: first compositor frame equals splash', b'appd: splash completed; restart disabled', b'splashd: stage=5 percent=100', b'splashd: handoff complete', b'scened: splash frame presented; takeover acknowledged']:
            assert marker in log, marker
        if test_input or validate_splash:
            assert len({digest for _, digest, _ in screenshots}) >= 3, 'No visible animation or transition'
            assert len(spinner_frames) >= 3, 'Spinner did not visibly animate'
            assert len(progress_frames) >= 2, 'Progress bar did not visibly advance'
        else:
            assert screenshots, 'No post-handoff desktop capture'
            assert desktop_visible, 'Styled desktop text was not visible after handoff'
        assert log.count(b'appd: graphics deadline profile applied') >= expected_profiles, 'Missing startup or replacement scheduling profile'
        for _, _, path in screenshots:
            data = path.read_bytes().split(b'\n', 3)[-1]
            assert any(data), f'Blank frame: {path}'
        print(f'Validated {len(screenshots)} graphical frames on {arch}')
        measured = re.search(rb'splashd: handoff complete frames=(\d+) render_us=(\d+)', log)
        if measured:
            frames, render_us = map(int, measured.groups())
            metrics = {'architecture': arch, 'width': width, 'height': height, 'hardware_performance_verified': False, 'frames': frames, 'render_us': render_us,
                       'mean_render_us': render_us / frames, 'captures': len(screenshots),
                       'spinner_variants': len(spinner_frames), 'progress_variants': len(progress_frames),
                       'input_host_latency_ms': hotplug.latencies_ms if test_input else {},
                       'input_latency_scope': 'QMP injection to serial observation; includes host polling and serial latency'}
            (output_dir / 'metrics.json').write_text(json.dumps(metrics, indent=2) + '\n')
            print(json.dumps(metrics))
    finally:
        (output_dir / 'serial.log').write_bytes(log)
        (output_dir / 'transplants.log').write_bytes(probe_log)
        if probe is not None and probe.poll() is None:
            probe.terminate()
            try:
                probe.wait(timeout=3)
            except subprocess.TimeoutExpired:
                probe.kill()
                probe.wait()
        if peer is not None:
            peer.close()
        listener.close()
        qmp.close()
        if debug is not None:
            debug.close()
        process.terminate()
        try:
            process.wait(timeout=3)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()

if performance:
    print("SCENED_NATIVE_FIXTURE_VERIFIED", flush=True)
