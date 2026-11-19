#!/usr/bin/env python3
"""Clean Linux command runner: temporary HOME, literal environment, no IPv4/IPv6.
Unix sockets remain available for local tool IPC. No third-party Python packages.
"""
import ctypes
import ctypes.util
import errno
import os
from pathlib import Path
import subprocess
import tempfile
import sys

class Comparison(ctypes.Structure):
    _fields_ = [('arg', ctypes.c_uint), ('op', ctypes.c_uint),
                ('a', ctypes.c_uint64), ('b', ctypes.c_uint64)]

lib = ctypes.CDLL(ctypes.util.find_library('seccomp') or 'libseccomp.so.2', use_errno=True)
lib.seccomp_init.argtypes = [ctypes.c_uint32]
lib.seccomp_init.restype = ctypes.c_void_p
lib.seccomp_syscall_resolve_name.argtypes = [ctypes.c_char_p]
lib.seccomp_syscall_resolve_name.restype = ctypes.c_int
lib.seccomp_rule_add_array.argtypes = [ctypes.c_void_p, ctypes.c_uint32, ctypes.c_int,
                                      ctypes.c_uint, ctypes.POINTER(Comparison)]
lib.seccomp_load.argtypes = [ctypes.c_void_p]
lib.seccomp_release.argtypes = [ctypes.c_void_p]
context = lib.seccomp_init(0x7fff0000)  # SCMP_ACT_ALLOW
if not context:
    raise RuntimeError('seccomp_init failed')
for domain in (2, 10):  # AF_INET, AF_INET6
    cmp = Comparison(0, 4, domain, 0)  # SCMP_CMP_EQ
    if lib.seccomp_rule_add_array(context, 0x50000 | errno.EPERM,
                                  lib.seccomp_syscall_resolve_name(b'socket'), 1, ctypes.byref(cmp)):
        raise RuntimeError('seccomp socket rule failed')
if lib.seccomp_load(context):
    raise RuntimeError('seccomp_load failed')
lib.seccomp_release(context)


def build_environment(home, window=False, cargo=False):
    """Only documented literals and our newly-created home reach the command."""
    env = {}
    profile = None
    build_keys = {'PATH', 'HOME', 'LANG', 'LC_ALL', 'XDG_RUNTIME_DIR'}
    window_keys = {'XDG_DATA_DIRS', 'GDK_BACKEND', 'HOH_WEBKIT_PROBE', 'LIBGL_ALWAYS_SOFTWARE',
                   '__GLX_VENDOR_LIBRARY_NAME', '__EGL_VENDOR_LIBRARY_FILENAMES',
                   'WEBKIT_DISABLE_COMPOSITING_MODE', 'GIO_USE_VFS'}
    cargo_keys = {'CARGO_BUILD_JOBS', 'CARGO_HOME', 'CARGO_TARGET_DIR', 'CARGO_NET_OFFLINE', 'RUSTUP_HOME', 'RUSTUP_TOOLCHAIN'}
    for line in Path(__file__).with_name('build-env.allow').read_text().splitlines():
        setting = line.split('#', 1)[0].strip()
        if not setting:
            continue
        if setting in ('[build]', '[window]', '[cargo]'):
            profile = setting
            continue
        if (profile == '[window]' and not window) or (profile == '[cargo]' and not cargo):
            continue
        name, value = setting.split('=', 1)
        allowed = {'[build]': build_keys, '[window]': window_keys, '[cargo]': cargo_keys}.get(profile, set())
        if name not in allowed or name in env:
            raise ValueError('invalid/duplicate build environment variable: ' + name)
        env[name] = value.replace('@HOME@', home)
    if set(env) != build_keys | (window_keys if window else set()) | (cargo_keys if cargo else set()):
        raise ValueError('incomplete build environment allow-list')
    if env['HOME'] != home or env['XDG_RUNTIME_DIR'] != home + '/runtime':
        raise ValueError('build HOME/runtime must be disposable')
    return env


if len(sys.argv) < 2:
    raise SystemExit('usage: offline.py COMMAND [ARG ...]')
with tempfile.TemporaryDirectory(prefix='hoh-ui-build-', dir='/tmp') as home:
    os.mkdir(home + '/runtime', mode=0o700)
    # env -i also documents the process boundary explicitly. No inherited values.
    env = build_environment(home, window='xvfb-run' in sys.argv[1:], cargo=Path(sys.argv[1]).name == 'cargo')
    command = ['/usr/bin/env', '-i', *[f'{k}={v}' for k, v in env.items()], *sys.argv[1:]]
    result = subprocess.run(command)
    raise SystemExit(result.returncode if result.returncode >= 0 else 128 - result.returncode)
