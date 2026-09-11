"""Place the sole SDL scanout window exactly on the Xvfb framebuffer.

Xvfb has no window manager, so SDL fullscreen/centering requests alone do not
establish the window geometry. Resize the actual X window and let SDL process
ConfigureNotify before comparing pixels in guest display coordinates.
"""
import ctypes as c


def place_scanout():
    x = c.CDLL("libX11.so.6")
    display = c.c_void_p
    window = c.c_ulong
    x.XOpenDisplay.argtypes = [c.c_char_p]
    x.XOpenDisplay.restype = display
    x.XDefaultRootWindow.argtypes = [display]
    x.XDefaultRootWindow.restype = window
    x.XQueryTree.argtypes = [display, window, c.POINTER(window), c.POINTER(window),
                            c.POINTER(c.POINTER(window)), c.POINTER(c.c_uint)]
    x.XGetGeometry.argtypes = [display, window, c.POINTER(window), c.POINTER(c.c_int),
                              c.POINTER(c.c_int), *([c.POINTER(c.c_uint)] * 4)]
    x.XMoveResizeWindow.argtypes = [display, window, c.c_int, c.c_int, c.c_uint, c.c_uint]
    x.XSync.argtypes = [display, c.c_int]
    x.XCloseDisplay.argtypes = [display]
    x.XFree.argtypes = [c.c_void_p]
    d = x.XOpenDisplay(None)
    if not d:
        raise RuntimeError("cannot open fixture display")
    children = c.POINTER(window)()
    try:
        root, parent, count = window(), window(), c.c_uint()
        if not x.XQueryTree(d, x.XDefaultRootWindow(d), c.byref(root), c.byref(parent),
                            c.byref(children), c.byref(count)):
            raise RuntimeError("cannot inspect fixture windows")
        candidates = []
        for i in range(count.value):
            left, top = c.c_int(), c.c_int()
            width, height, border, depth = (c.c_uint() for _ in range(4))
            if x.XGetGeometry(d, children[i], c.byref(root), c.byref(left), c.byref(top),
                              c.byref(width), c.byref(height), c.byref(border), c.byref(depth)):
                if width.value >= 320 and height.value >= 240:
                    candidates.append(children[i])
        if len(candidates) != 1:
            raise RuntimeError("expected one SDL scanout window, found " + str(len(candidates)))
        x.XMoveResizeWindow(d, candidates[0], 0, 0, 800, 600)
        x.XSync(d, 0)
    finally:
        if children:
            x.XFree(children)
        x.XCloseDisplay(d)
