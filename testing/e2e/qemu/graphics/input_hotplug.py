"""Bounded QEMU input attach and surprise-removal fixture shared by both hosts."""
import re
import time

HOTPLUG_ROOT_PORT = "pcie-root-port,id=input-slot,chassis=1,slot=1"
# Attachment includes reading, verifying and linking the installed driver and
# shared libraries under TCG. Keep that functional budget separate from event
# delivery/removal, and from all guest scheduling and migration deadlines.
ATTACH_TIMEOUT_SECONDS = 90
EVENT_TIMEOUT_SECONDS = 30


class InputHotplug:
    def __init__(self, request):
        self.request = request
        self.phase = 0
        self.offset = 0
        self.endpoint = 0
        self.node = 0
        self.deadline = 0
        self.latencies_ms = {}

    def key(self, down):
        self.request("input-send-event", {"events": [{"type": "key", "data": {
            "key": {"type": "qcode", "data": "c"}, "down": down}}]})

    def poll(self, log):
        if self.phase == 4:
            return True
        now = time.monotonic()
        if self.phase == 0:
            self.offset = len(log)
            self.request("device_add", {"driver": "virtio-keyboard-pci", "id": "hotkey", "bus": "input-slot"})
            self.added_at = time.monotonic()
            self.phase = 1
            self.deadline = self.added_at + ATTACH_TIMEOUT_SECONDS
        tail = log[self.offset:]
        if self.phase == 1:
            match = re.search(rb"scened: input device attached node=(\d+) endpoint=(\d+)", tail)
            if match:
                self.node, self.endpoint = map(int, match.groups())
                self.latencies_ms["device_add_to_compositor_attachment"] = (time.monotonic() - self.added_at) * 1000
                self.key(True)
                self.sent_at = time.monotonic()
                self.phase = 2
                self.deadline = self.sent_at + EVENT_TIMEOUT_SECONDS
        elif self.phase == 2:
            pattern = rb"input-fixture: view=0 kind=2 phase=0 code=46 state=1[^\n]*device=" + str(self.endpoint).encode() + rb"(?:\n|\r)"
            if re.search(pattern, tail):
                self.latencies_ms["host_inject_to_client_log"] = (time.monotonic() - self.sent_at) * 1000
                # Surprise removal: unrealize the actual PCI device immediately,
                # without requiring a guest ACPI eject acknowledgement. The PCI
                # function disappears and device DMA stops in QEMU.
                self.request("qom-set", {"path": "/machine/peripheral/hotkey", "property": "realized", "value": False})
                self.removed_at = time.monotonic()
                self.phase = 3
                self.deadline = self.removed_at + EVENT_TIMEOUT_SECONDS
        elif self.phase == 3:
            pattern = rb"input-fixture: view=0 kind=2 phase=0 code=46 state=0[^\n]*device=" + str(self.endpoint).encode() + rb"(?:\n|\r)"
            removed = f"appd: input device removed node={self.node}".encode()
            canceled = re.search(pattern, tail) is not None
            retired = removed in tail
            elapsed = (time.monotonic() - self.removed_at) * 1000
            if canceled:
                self.latencies_ms.setdefault("surprise_removal_to_key_cancellation", elapsed)
            if retired:
                self.latencies_ms.setdefault("surprise_removal_to_registry_retirement", elapsed)
            if retired and canceled:
                self.key(False)
                self.phase = 4
                return True
        if now >= self.deadline:
            inventory = self.request("query-pci")
            raise AssertionError(f"input hotplug phase {self.phase} timed out; PCI={inventory}:\n" + tail.decode(errors="replace"))
        return False


class InputDisconnect:
    """Close one real client with a held key; preserve the other client."""
    def __init__(self, request):
        self.request = request
        self.phase = 0

    def events(self, events):
        self.request("input-send-event", {"events": events})

    @staticmethod
    def key(name, down):
        return {"type": "key", "data": {"key": {"type": "qcode", "data": name}, "down": down}}

    def poll(self, log):
        if self.phase == 3:
            return True
        now = time.monotonic()
        if self.phase == 0:
            self.offset = len(log)
            self.deadline = now + 30
            self.events([{"type": "abs", "data": {"axis": "x", "value": 26000}},
                         {"type": "abs", "data": {"axis": "y", "value": 10000}},
                         {"type": "btn", "data": {"button": "left", "down": True}}])
            self.events([{"type": "btn", "data": {"button": "left", "down": False}}])
            self.phase = 1
        tail = log[self.offset:]
        if self.phase == 1 and b"input-fixture: view=1 kind=1 phase=1" in tail:
            self.events([self.key("d", True), self.key("f8", True), self.key("f8", False)])
            self.phase = 2
        if self.phase == 2 and b"input-fixture: disconnected view retired lease and identity; surviving client responsive" in tail:
            assert b"input-fixture: view=1 kind=2 phase=0 code=32 state=1" in tail
            assert b"input-fixture: view=0 kind=2 phase=0 code=32" not in tail
            self.events([self.key("d", False)])
            self.phase = 3
            return True
        assert now < self.deadline, "client disconnect timed out:\n" + tail.decode(errors="replace")
        return False
