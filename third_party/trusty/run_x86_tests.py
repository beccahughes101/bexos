"""Exercise the standalone runner's ownership of real child processes."""
from pathlib import Path
import os
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

import run_x86


class ProcessOwnershipTests(unittest.TestCase):
    def exercise(self, qemu_program, markers, timeout, failure=None, proxy_failure=False):
        children = []
        spawn = subprocess.Popen
        with tempfile.TemporaryDirectory() as temporary:
            work = Path(temporary)

            def child(command, **kwargs):
                if command[0] == 'rpmb-test':
                    code = ('raise SystemExit(7)' if proxy_failure else
                            'import socket, time; s=socket.socket(socket.AF_UNIX); '
                            f's.bind({str(work / "rpmb.sock")!r}); time.sleep(60)')
                else:
                    self.assertEqual(command[0], 'qemu-system-x86_64')
                    self.assertIn('q35,accel=tcg', command)
                    code = qemu_program
                process = spawn([sys.executable, '-u', '-c', code], **kwargs)
                children.append(process)
                return process

            with patch.object(run_x86.subprocess, 'Popen', side_effect=child), patch.object(run_x86.os, 'write'):
                if failure:
                    with self.assertRaises(failure):
                        run_x86.boot(Path('boot.elf'), Path('rpmb-test'), Path('state'), work, markers, timeout)
                else:
                    run_x86.boot(Path('boot.elf'), Path('rpmb-test'), Path('state'), work, markers, timeout)
            self.assertFalse((work / 'rpmb.sock').exists())
        for child in children:
            self.assertIsNotNone(child.poll())
            with self.assertRaises(ChildProcessError):
                os.waitpid(child.pid, os.WNOHANG)

    def test_success_reaps_both_children(self):
        self.exercise('import time; print("accepted"); time.sleep(60)', ['accepted'], 5)

    def test_timeout_reaps_both_children(self):
        self.exercise('import time; time.sleep(60)', ['accepted'], .05, TimeoutError)

    def test_early_exit_is_failure_and_reaps_proxy(self):
        self.exercise('raise SystemExit(3)', ['accepted'], 5, RuntimeError)

    def test_proxy_failure_does_not_start_qemu(self):
        self.exercise('raise AssertionError("must not start")', ['accepted'], 5, RuntimeError, True)

    def test_guest_panic_is_failure(self):
        self.exercise('import time; print("panic (caller 0x123): failed"); time.sleep(60)', ['accepted'], 5, RuntimeError)


if __name__ == '__main__':
    unittest.main()
