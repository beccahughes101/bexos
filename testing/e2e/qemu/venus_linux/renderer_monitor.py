#!/usr/bin/python3
"""Fixture-only render-server crash diagnostics, without tracing syscalls.

Follow child creation and fatal signals so a renderer process cannot disappear
without evidence. Signals are delivered unchanged. EXITKILL and the enclosing
VM deadline bound cleanup even if the diagnostic monitor itself fails.
"""
import ctypes
import os
from pathlib import Path
import shutil
import signal
import sys
from renderer_stack import capture


libc = ctypes.CDLL(None, use_errno=True)
libc.ptrace.restype = ctypes.c_long
libc.ptrace.argtypes = [ctypes.c_uint, ctypes.c_int, ctypes.c_void_p, ctypes.c_void_p]


def trace(request, pid, data=0):
    if libc.ptrace(request, pid, None, data) < 0:
        raise OSError(ctypes.get_errno(), 'renderer lifecycle tracing')


server = shutil.which('virgl_render_server') or next((str(path) for path in (
    Path('/usr/libexec/virgl_render_server'),
    Path('/usr/lib/virglrenderer/virgl_render_server'),
) if path.is_file()), None)
assert server, 'pinned render server missing'
child = os.fork()
if child == 0:
    trace(0, 0)
    os.kill(os.getpid(), signal.SIGSTOP)
    os.execv(server, [server, *sys.argv[1:]])
os.waitpid(child, 0)
# Fork, vfork, clone, exec, exit, and kill tracees if this monitor dies.
trace(0x4200, child, 2 | 4 | 8 | 16 | 64 | 0x100000)
trace(7, child)
while True:
    try:
        pid, status = os.waitpid(-1, 0x40000000)
    except ChildProcessError:
        break
    if os.WIFEXITED(status) or os.WIFSIGNALED(status):
        print('VENUS_RENDERER_EXIT', {'pid': pid, 'status': status}, flush=True)
        continue
    if not os.WIFSTOPPED(status):
        continue
    stopped = os.WSTOPSIG(status)
    event = status >> 16
    if stopped in (signal.SIGSEGV, signal.SIGABRT, signal.SIGBUS, signal.SIGILL, signal.SIGSYS):
        # /proc/<tid>/maps works for any member of the thread group.
        capture(pid, pid, stopped=True)
        print('VENUS_RENDERER_FATAL', {'pid': pid, 'signal': stopped}, flush=True)
    # Ptrace event traps and the initial child SIGSTOP are synthetic.
    try:
        trace(7, pid, 0 if event or stopped == signal.SIGSTOP else stopped)
    except ProcessLookupError:
        # Group exit/SIGKILL can remove a tracee between waitpid and CONT.
        # Reap the remaining exit notifications instead of crashing this
        # monitor: EXITKILL would otherwise kill unrelated live contexts.
        continue
