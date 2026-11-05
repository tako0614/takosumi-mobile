#!/usr/bin/env python3
"""Read the actual X11 resource owner's PID through the XRes 1.2 protocol."""
import ctypes as c
import ctypes.util
import json
import sys


class Spec(c.Structure):
    _fields_ = [('client', c.c_ulong), ('mask', c.c_uint)]


class Value(c.Structure):
    _fields_ = [('spec', Spec), ('length', c.c_long), ('value', c.c_void_p)]


def owner(window):
    if not 0 < window < 2 ** 32:
        raise ValueError('An X11 window resource must be a positive 32-bit ID')
    x11 = c.CDLL(ctypes.util.find_library('X11') or 'libX11.so.6')
    res = c.CDLL(ctypes.util.find_library('XRes') or 'libXRes.so.1')
    x11.XOpenDisplay.argtypes = [c.c_char_p]
    x11.XOpenDisplay.restype = c.c_void_p
    x11.XCloseDisplay.argtypes = [c.c_void_p]
    res.XResQueryExtension.argtypes = [c.c_void_p, c.POINTER(c.c_int), c.POINTER(c.c_int)]
    res.XResQueryExtension.restype = c.c_int
    res.XResQueryVersion.argtypes = [c.c_void_p, c.POINTER(c.c_int), c.POINTER(c.c_int)]
    res.XResQueryVersion.restype = c.c_int
    res.XResQueryClientIds.argtypes = [c.c_void_p, c.c_long, c.POINTER(Spec), c.POINTER(c.c_long), c.POINTER(c.POINTER(Value))]
    res.XResQueryClientIds.restype = c.c_int
    res.XResGetClientPid.argtypes = [c.POINTER(Value)]
    res.XResGetClientPid.restype = c.c_int
    res.XResClientIdsDestroy.argtypes = [c.c_long, c.POINTER(Value)]
    res.XResClientIdsDestroy.restype = None
    display = x11.XOpenDisplay(None)
    if not display:
        raise RuntimeError('An existing owned X11 display is required')
    count, values = c.c_long(), c.POINTER(Value)()
    try:
        event, error, major, minor = (c.c_int() for _ in range(4))
        if not res.XResQueryExtension(display, c.byref(event), c.byref(error)):
            raise RuntimeError('XRes extension is unavailable')
        if not res.XResQueryVersion(display, c.byref(major), c.byref(minor)) or (major.value, minor.value) < (1, 2):
            raise RuntimeError('XRes client PID queries require protocol 1.2')
        # Exact ABI from the installed X11/extensions/XRes.h. The requested
        # mask is XRES_CLIENT_ID_PID_MASK; no window property is trusted.
        spec = Spec(window, 2)
        # Unlike the older Bool-style queries, this API returns Success (0).
        if res.XResQueryClientIds(display, 1, c.byref(spec), c.byref(count), c.byref(values)) != 0:
            raise RuntimeError('XRes client PID query failed')
        if count.value != 1 or not values:
            raise RuntimeError('No unique X11 resource owner')
        pid = res.XResGetClientPid(c.byref(values[0]))
        if pid <= 0:
            raise RuntimeError('X11 resource has no available PID')
        return {'window': window, 'pid': pid, 'method': 'XResQueryClientIds/PID', 'protocol': [major.value, minor.value]}
    finally:
        if values:
            res.XResClientIdsDestroy(count.value, values)
        x11.XCloseDisplay(display)


if __name__ == '__main__':
    if len(sys.argv) != 2:
        raise SystemExit('Usage: x11-owner.py <decimal X11 window ID>')
    print(json.dumps(owner(int(sys.argv[1]))))
