#!/usr/bin/env python3
"""Run an already-built window with a disposable home, X server and bus; no IP sockets (unless HOH_WINDOW_NET=1)."""
import os
from pathlib import Path
import subprocess
import sys
import tempfile

root = Path(__file__).resolve().parents[2]
with tempfile.TemporaryDirectory(prefix='hoh-webkitgtk-') as home:
    runtime = Path(home) / 'runtime'
    runtime.mkdir(mode=0o700)
    env = {key: value for key, value in os.environ.items() if key not in
           ('WAYLAND_DISPLAY', 'DBUS_SESSION_BUS_ADDRESS', 'LD_LIBRARY_PATH', 'GSETTINGS_SCHEMA_DIR')}
    env.update(HOME=home, XDG_CONFIG_HOME=home+'/config', XDG_CACHE_HOME=home+'/cache',
               XDG_DATA_HOME=home+'/data', XDG_RUNTIME_DIR=str(runtime),
               XDG_DATA_DIRS='/usr/local/share:/usr/share', GDK_BACKEND='x11', HOH_WEBKIT_PROBE='1',
               LIBGL_ALWAYS_SOFTWARE='1', __GLX_VENDOR_LIBRARY_NAME='mesa',
               __EGL_VENDOR_LIBRARY_FILENAMES='/usr/share/glvnd/egl_vendor.d/50_mesa.json',
               WEBKIT_DISABLE_COMPOSITING_MODE='1', GIO_USE_VFS='local')
    # QA on a model (mock on 127.0.0.1 or live): HOH_WINDOW_NET=1 keeps IP sockets; HOH_WINDOW_TIMEOUT for long sessions.
    net = os.environ.get('HOH_WINDOW_NET') == '1'
    guard = [] if net else ['python3', str(root/'ui/scripts/offline.py')]
    result = subprocess.run([*guard, 'timeout', os.environ.get('HOH_WINDOW_TIMEOUT', '60'), 'xvfb-run', '-a', '-e', '/tmp/hoh-ui2-xvfb.log', '-s',
                             '-screen 0 1200x800x24 -nolisten tcp -extension GLX', 'dbus-run-session', *sys.argv[1:]], env=env)
    sys.exit(result.returncode)
