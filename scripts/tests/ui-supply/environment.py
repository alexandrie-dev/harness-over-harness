#!/usr/bin/env python3
"""Exercise real launchers with an environment printer, never a build tool."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[3]
ALLOWED = {'PATH', 'HOME', 'LANG', 'LC_ALL', 'XDG_RUNTIME_DIR'}
with tempfile.TemporaryDirectory(prefix='hoh-env-test-') as directory:
    root = Path(directory)
    scripts = root / 'ui/scripts'
    scripts.mkdir(parents=True)
    for filename in ('offline.py', 'build-env.allow'):
        shutil.copyfile(ROOT / 'ui/scripts' / filename, scripts / filename)
    # Substitute only the final build command in a copy of the real shell launcher.
    source = (ROOT / 'ui/scripts/build.sh').read_text()
    assert source.count('pnpm run build:offline') == 1
    (scripts / 'build.sh').write_text(source.replace('pnpm run build:offline', '/usr/bin/env'))
    bad_shell = root / 'bad-shell.sh'
    bad_shell.write_text('echo injected > "' + str(root / 'injected') + '"\n')
    poisoned = os.environ | {
        'VP_RUN_NODE_CLIENT_PATH': '/outside/evil.js', 'NODE_OPTIONS': '--require=/outside/evil.js',
        'npm_config_userconfig': '/outside/npmrc', 'NPM_CONFIG_REGISTRY': 'https://outside.invalid',
        'PYTHONPATH': '/outside/python', 'LD_PRELOAD': '', 'SECRET_TOKEN': 'must-not-leak',
        'BASH_ENV': str(bad_shell), 'CDPATH': '/outside', 'HOME': '/outside/home', 'LANG': 'bad_locale', 'PATH': '/outside/bin',
    }
    printed = subprocess.check_output(['/bin/bash', '-p', str(scripts / 'build.sh')], env=poisoned, text=True)
    env = dict(line.split('=', 1) for line in printed.splitlines())
    assert set(env) == ALLOWED, env
    assert env['LANG'] == env['LC_ALL'] == 'C.UTF-8'
    assert env['PATH'].startswith(str(ROOT / 'ui/scripts/tools') + ':/home/save/.nvm/versions/node/v20.20.2/bin:')
    assert env['HOME'].startswith('/tmp/hoh-ui-build-') and not Path(env['HOME']).exists()
    assert env['XDG_RUNTIME_DIR'] == env['HOME'] + '/runtime'
    assert not (root / 'injected').exists()
    # Direct offline.py call, with both hostile Node variables and a real child.
    code = '''import json,os,socket
from pathlib import Path
assert Path.home().is_dir()
assert Path(os.environ['XDG_RUNTIME_DIR']).is_dir()
for family in (socket.AF_INET, socket.AF_INET6):
 try: socket.socket(family)
 except PermissionError: pass
 else: raise AssertionError('IP socket was allowed')
a,b=socket.socketpair(); a.close(); b.close()
print(json.dumps(dict(os.environ)))'''
    result = subprocess.check_output(['/usr/bin/python3', '-I', str(ROOT / 'ui/scripts/offline.py'),
                                      '/usr/bin/python3', '-I', '-c', code], env=poisoned, text=True)
    direct = json.loads(result)
    assert set(direct) == ALLOWED, direct
    assert direct['HOME'] != env['HOME'] and not Path(direct['HOME']).exists()
    # Package-script and Tauri entrypoints must reach the same boundary, even
    # when BASH_ENV is hostile. No Node, pnpm, cargo or browser is started.
    package = json.loads((ROOT / 'ui/package.json').read_text())
    tauri = json.loads((ROOT / 'apps/hoh-desktop/tauri.conf.json').read_text())
    desktop = root / 'apps/hoh-desktop'
    desktop.mkdir(parents=True)
    for command, cwd in [(package['scripts']['build'], root / 'ui'),
                         (tauri['build']['beforeBuildCommand'], desktop)]:
        printed = subprocess.check_output(['/bin/sh', '-c', command], cwd=cwd, env=poisoned, text=True)
        entry_env = dict(line.split('=', 1) for line in printed.splitlines())
        assert set(entry_env) == ALLOWED, entry_env
        assert not (root / 'injected').exists()
    raw_entry = package['scripts']['build:offline']
    prefix, payload, suffix = raw_entry.split("'")
    assert prefix == '/usr/bin/env -i /usr/bin/python3 -I scripts/offline.py /bin/sh -c ' and not suffix
    printed = subprocess.check_output(['/bin/sh', '-c', prefix + "'/usr/bin/env'"],
                                      cwd=root / 'ui', env=poisoned, text=True)
    entry_env = dict(line.split('=', 1) for line in printed.splitlines())
    assert ALLOWED <= set(entry_env) <= ALLOWED | {'PWD'}, entry_env
    assert entry_env.get('PWD', str(root / 'ui')) == str(root / 'ui')
    # Preserve the existing native probe's fixed Mesa settings, without running it
    # or forwarding the caller's environment. The extra argument selects its profile.
    printed = subprocess.check_output(['/usr/bin/python3', '-I', str(ROOT / 'ui/scripts/offline.py'),
                                      '/usr/bin/python3', '-I', '-c', 'import json,os; print(json.dumps(dict(os.environ)))',
                                      'xvfb-run'], env=poisoned | {'HOH_WEBKIT_PROBE': 'external'}, text=True)
    window_env = json.loads(printed)
    assert window_env['HOH_WEBKIT_PROBE'] == '1' and window_env['GDK_BACKEND'] == 'x11'
    assert window_env['__GLX_VENDOR_LIBRARY_NAME'] == 'mesa'
    assert 'NODE_OPTIONS' not in window_env and 'VP_RUN_NODE_CLIENT_PATH' not in window_env
    # Never invoke cargo: this substitute only prints the fixed native profile.
    fake_cargo = root / 'cargo'
    fake_cargo.write_text('#!/usr/bin/python3 -I\nimport json,os\nprint(json.dumps(dict(os.environ)))\n')
    fake_cargo.chmod(0o755)
    printed = subprocess.check_output(['/usr/bin/python3', '-I', str(ROOT / 'ui/scripts/offline.py'),
                                      str(fake_cargo)], env=poisoned | {'CARGO_HOME': '/outside/cargo'}, text=True)
    cargo_env = json.loads(printed)
    assert cargo_env['CARGO_BUILD_JOBS'] == '2'
    assert cargo_env['CARGO_HOME'] == '/mnt/toshiba/alexandrie/hoh/cargo-home-ui2'
    assert cargo_env['CARGO_TARGET_DIR'] == '/mnt/toshiba/alexandrie/hoh/target-ui2'
    assert cargo_env['CARGO_NET_OFFLINE'] == 'true' and 'NODE_OPTIONS' not in cargo_env
    assert cargo_env['RUSTUP_HOME'] == '/home/save/.rustup'
    assert cargo_env['RUSTUP_TOOLCHAIN'] == '1.97.1-x86_64-unknown-linux-gnu'
print('PASS clean environment: shell + offline runner, hostile variables absent, private HOME cleaned, IP denied')
