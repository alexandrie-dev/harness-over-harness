#!/usr/bin/env bash
# Record release-window startup against 5 and 30 synthetic saved conversations.
# The video and engine proxy log are evidence, not an automatic paint assertion.
set -euo pipefail
if [[ ${1:-} == --help || ${1:-} == -h ]]; then
  cat <<'HELP'
Usage: scripts/perf-window.sh [new /mnt/toshiba/alexandrie/hoh/qa/u5-* directory]
RUNS=3 COUNTS="5 30" CAPTURE_SECONDS=45 RECORD=1 control the captures.
SEND_AT=seconds sends a test message using fixed coordinates; OPEN_AT=seconds opens the first sidebar chat.
RESTART=1 reopens the same data; RESTART_PENDING=1 additionally clears onboarding completion for that synthetic experiment.
The script records 10 fps video and timestamped engine frames. Inspect frames and verify each automated action before using the numbers.
HELP
  exit 0
fi
ROOT=$(cd "$(dirname "$0")/.." && pwd)
QA=/mnt/toshiba/alexandrie/hoh/qa
DESKTOP=/mnt/toshiba/alexandrie/hoh/release/hoh-desktop-41f4fee9
ENGINE=/mnt/toshiba/alexandrie/hoh/release/hoh-41f4fee9
OUT=${1:-$QA/u5-window-$(date +%Y%m%d-%H%M%S)-$$}
RUNS=${RUNS:-3}
CAPTURE_SECONDS=${CAPTURE_SECONDS:-45}
SEND_AT=${SEND_AT:-0}
OPEN_AT=${OPEN_AT:-0}
RESTART=${RESTART:-0}
RESTART_PENDING=${RESTART_PENDING:-0}
RECORD=${RECORD:-1}
COUNTS=${COUNTS:-"5 30"}
[[ $OUT == "$QA"/u5-* && ! -e $OUT ]] || { echo 'Output must be a new qa/u5-* directory' >&2; exit 1; }
for cmd in Xvfb ffmpeg ffprobe python3 dbus-run-session metacity pactl setsid zstd xdotool; do command -v "$cmd" >/dev/null || { echo "Missing $cmd" >&2; exit 1; }; done
[[ -x $DESKTOP && -x $ENGINE ]] || { echo 'Missing release binaries' >&2; exit 1; }
mkdir -p "$OUT"/{xvfb-home,xvfb-runtime,xvfb-config,xvfb-cache,xvfb-data,xvfb-state}
OUT=$(realpath "$OUT")
chmod 700 "$OUT/xvfb-home" "$OUT/xvfb-runtime"
display=
for n in {120..199}; do
  if [[ ! -S /tmp/.X11-unix/X$n && ! -e /tmp/.X$n-lock ]]; then display=:$n; break; fi
done
[[ -n $display ]] || { echo 'No free X display' >&2; exit 1; }
xpid= apppid= recpid= actionpid=
cleanup() {
  if [[ -n $apppid ]] && kill -0 "$apppid" 2>/dev/null; then kill -TERM -- "-$apppid" 2>/dev/null || true; wait "$apppid" 2>/dev/null || true; fi
  for pid in "$actionpid" "$recpid" "$xpid"; do
    if [[ -n $pid ]] && kill -0 "$pid" 2>/dev/null; then kill -TERM "$pid" 2>/dev/null || true; wait "$pid" 2>/dev/null || true; fi
  done
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
env -u DBUS_SESSION_BUS_ADDRESS HOME="$OUT/xvfb-home" CLAUDE_CONFIG_DIR="$OUT/xvfb-home/claude" CODEX_HOME="$OUT/xvfb-home/codex" XDG_CONFIG_HOME="$OUT/xvfb-config" XDG_CACHE_HOME="$OUT/xvfb-cache" XDG_DATA_HOME="$OUT/xvfb-data" XDG_STATE_HOME="$OUT/xvfb-state" XDG_RUNTIME_DIR="$OUT/xvfb-runtime" PULSE_SERVER=unix:/nonexistent PIPEWIRE_REMOTE=/nonexistent ALSA_CONFIG_PATH=/dev/null GTK_USE_PORTAL=0 NO_AT_BRIDGE=1 Xvfb "$display" -screen 0 1440x900x24 -nolisten tcp -extension GLX >"$OUT/xvfb.log" 2>&1 & xpid=$!
for _ in {1..100}; do [[ -S /tmp/.X11-unix/X${display#:} ]] && break; kill -0 "$xpid" 2>/dev/null || exit 1; sleep .05; done
[[ -S /tmp/.X11-unix/X${display#:} ]] || { echo 'Xvfb did not start' >&2; exit 1; }
for count in $COUNTS; do
  [[ $count == 1 || $count == 5 || $count == 30 ]] || { echo "Invalid conversation count: $count" >&2; exit 1; }
  for ((run=1; run<=RUNS; run++)); do
    dir=$OUT/n$count-run$run
    mkdir -p "$dir"/{agents,bin,cache,config/local.hoh.desktop,data/local.hoh.desktop/localstorage,data/local.hoh.desktop/engine/sessions,home/Documents/HoH,logs,runtime,state,claude-starts}
    chmod 700 "$dir/agents" "$dir/runtime" "$dir/home"
    # Even capture/input helpers have no access to the user's session bus or audio.
    isolated=(env -u DBUS_SESSION_BUS_ADDRESS DISPLAY="$display" HOME="$dir/home" CLAUDE_CONFIG_DIR="$dir/home/claude-config" CODEX_HOME="$dir/home/codex-config" XDG_CONFIG_HOME="$dir/config" XDG_CACHE_HOME="$dir/cache" XDG_DATA_HOME="$dir/data" XDG_STATE_HOME="$dir/state" XDG_RUNTIME_DIR="$dir/runtime" PULSE_SERVER=unix:/nonexistent PIPEWIRE_REMOTE=/nonexistent ALSA_CONFIG_PATH=/dev/null GTK_USE_PORTAL=0 NO_AT_BRIDGE=1)
    xdo() { "${isolated[@]}" xdotool "$@"; }
    python3 - "$ROOT/apps/hoh/tests/window_executor.rs" "$dir" "$count" "$ENGINE" <<'PY'
import json, os, pathlib, sqlite3, subprocess, sys, time, uuid
source=pathlib.Path(sys.argv[1]).read_text(); d=pathlib.Path(sys.argv[2]); count=int(sys.argv[3]); engine=sys.argv[4]
fake=source.split('const FAKE_CLAUDE: &str = r##"',1)[1].split('"##;',1)[0].replace('@STARTS@',str(d/'claude-starts')).replace('@VERSION@','2.0.31')
(d/'bin/claude').write_text(fake); (d/'bin/claude').chmod(0o755)
(d/'bin/codex').write_text('#!/bin/sh\nexit 1\n'); (d/'bin/codex').chmod(0o755)
# Keep the benchmark past first-run setup while retaining T3's default settings.
settings=json.dumps({'onboardingCompletedAt':'2026-01-01T00:00:00.000Z'}).encode('utf-16le')
target=sqlite3.connect(d/'data/local.hoh.desktop/localstorage/tauri_localhost_0.localstorage')
target.execute('CREATE TABLE ItemTable (key TEXT UNIQUE ON CONFLICT REPLACE, value BLOB NOT NULL ON CONFLICT FAIL)')
target.execute('INSERT INTO ItemTable VALUES (?,?)',('t3code:client-settings:v1',settings))
target.execute('INSERT INTO ItemTable VALUES (?,?)',('t3code:theme','hoh'.encode('utf-16le')))
target.execute('INSERT INTO ItemTable VALUES (?,?)',('t3code:theme-appearance-mode','dark'.encode('utf-16le')))
target.commit(); target.close()
cwd=str(d/'home/Documents/HoH'); project='--'+cwd.lstrip('/').replace('/','-')+'--'; now=int(time.time()*1000)-1000
identity=os.stat(cwd)
for n in range(count):
 sid=str(uuid.uuid4()); folder=d/'data/local.hoh.desktop/engine/sessions'/project/sid; folder.mkdir(parents=True)
 def event(kind, seq, data, **extra): return {'type':kind,'seq':seq,'time':now+seq,'data':data,**extra}
 records=[
  {'type':'session','version':4,'id':sid,'createdAt':now,'cwd':cwd,'isSeeded':False,'delegationDepth':0},
  event('plugin:external-harness/session',0,{'harness':'claude','nativeSessionId':None,'model':'sonnet'},ignorable=True),
  event('turn/start',1,{'turn':1}),
  event('step/start',2,{'turn':1,'step':1}),
  event('user/message',3,{'content':[{'type':'text','text':f'Synthetic chat {n+1}'}],'source':{'kind':'user'},'role':'user','id':str(uuid.uuid4())},surfaceOp='append'),
  event('working-directory/change',4,{'cwd':cwd,'identity':{'path':cwd,'dev':identity.st_dev,'ino':identity.st_ino}}),
  event('plugin:external-harness/session',5,{'harness':'claude','nativeSessionId':str(uuid.uuid4()),'model':'sonnet'},ignorable=True),
  event('plugin:external-harness/launch',6,{'runtimeMode':'full','interactionMode':'default'},ignorable=True),
  event('assistant/message',7,{'turn':1,'step':1,'stream':[],'message':{'role':'assistant','content':[{'type':'text','text':'done'}],'source':{'kind':'model','provider':'claude','model':'sonnet'},'id':str(uuid.uuid4())}},surfaceOp='append'),
  event('step/end',8,{'turn':1,'step':1}),
  event('turn/end',9,{'turn':1,'reason':{'kind':'completed'}}),
 ]
 with (folder/'session.v4.jsonl.zstd').open('wb') as out:
  for record in records: out.write(subprocess.run(['zstd','-q','-c'],input=(json.dumps(record)+'\n').encode(),stdout=subprocess.PIPE,check=True).stdout)
# The proxy preserves the stdio protocol and records only method names, response ids and times.
proxy='''#!/usr/bin/python3 -I
import json, os, subprocess, sys, threading, time
log=open(os.environ['U5_PROXY_LOG'], 'a', buffering=1)
child=subprocess.Popen([%r,*sys.argv[1:]],stdin=subprocess.PIPE,stdout=subprocess.PIPE)
methods={}
def incoming():
 for line in sys.stdin.buffer:
  try:
   frame=json.loads(line); method=frame.get('method'); ident=frame.get('id')
   if method and ident is not None: methods[str(ident)]=method
   if method: log.write(json.dumps({'ns':time.time_ns(),'event':'request','method':method,'id':ident})+'\\n')
  except Exception: pass
  try: child.stdin.write(line); child.stdin.flush()
  except BrokenPipeError: break
 try: child.stdin.close()
 except BrokenPipeError: pass
threading.Thread(target=incoming,daemon=True).start()
for line in child.stdout:
 try:
  frame=json.loads(line); ident=frame.get('id')
  if frame.get('method') and ident is None: log.write(json.dumps({'ns':time.time_ns(),'event':'notification','method':frame['method']})+'\\n')
  if ident is not None and 'method' not in frame:
   result=frame.get('result'); log.write(json.dumps({'ns':time.time_ns(),'event':'response','method':methods.pop(str(ident),None),'id':ident,'count':len(result.get('sessions',result.get('events',[]))) if isinstance(result,dict) and ('sessions' in result or 'events' in result) else None,'error':frame.get('error',{}).get('code')})+'\\n')
 except Exception: pass
 try: sys.stdout.buffer.write(line); sys.stdout.buffer.flush()
 except BrokenPipeError: break
child.wait()
''' % engine
(d/'bin/engine-proxy').write_text(proxy); (d/'bin/engine-proxy').chmod(0o755)
PY
    printf 'XDG_DOCUMENTS_DIR="%s"\n' "$dir/home/Documents" >"$dir/config/user-dirs.dirs"
    printf '{"folder":"%s"}\n' "$dir/home/Documents/HoH" >"$dir/config/local.hoh.desktop/workspace.json"
    cat >"$dir/runtime/bus.conf" <<BUS
<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN" "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig><type>session</type><listen>unix:dir=$dir/runtime</listen><auth>EXTERNAL</auth><policy context="default"><allow send_destination="*" eavesdrop="true"/><allow eavesdrop="true"/><allow own="*"/></policy></busconfig>
BUS
    pactl list short sink-inputs >"$dir/logs/sink-inputs-before.log" 2>&1 || true
    # Record before launching the desktop; capture.log records the first input packet wall-clock PTS.
    if [[ $RECORD == 1 ]]; then
      "${isolated[@]}" ffmpeg -nostdin -loglevel info -debug_ts -use_wallclock_as_timestamps 1 -f x11grab -framerate 10 -video_size 1440x900 -i "$display" -t "$CAPTURE_SECONDS" -c:v ffv1 "$dir/capture.mkv" >"$dir/logs/capture.log" 2>&1 & recpid=$!
    else
      sleep "$CAPTURE_SECONDS" & recpid=$!
    fi
    sleep 1
    setsid env -u DBUS_SESSION_BUS_ADDRESS DISPLAY="$display" HOME="$dir/home" CLAUDE_CONFIG_DIR="$dir/home/claude-config" CODEX_HOME="$dir/home/codex-config" XDG_CONFIG_HOME="$dir/config" XDG_CACHE_HOME="$dir/cache" XDG_DATA_HOME="$dir/data" XDG_STATE_HOME="$dir/state" XDG_RUNTIME_DIR="$dir/runtime" DSH_AGENTS_HOME="$dir/agents" HOH_ENGINE_PATH="$dir/bin/engine-proxy" U5_PROXY_LOG="$dir/logs/engine-frames.jsonl" PATH="$dir/bin:$PATH" GDK_BACKEND=x11 GIO_USE_VFS=local LIBGL_ALWAYS_SOFTWARE=1 WEBKIT_DISABLE_COMPOSITING_MODE=1 PULSE_SERVER=unix:/nonexistent PIPEWIRE_REMOTE=/nonexistent ALSA_CONFIG_PATH=/dev/null GTK_USE_PORTAL=0 NO_AT_BRIDGE=1 \
      dbus-run-session --config-file="$dir/runtime/bus.conf" -- bash -c '
        metacity >"$1/logs/metacity.log" 2>&1 & wm=$!
        trap '\''kill "$wm" 2>/dev/null || true; wait "$wm" 2>/dev/null || true'\'' EXIT
        date +%s.%N >"$1/logs/process-start.txt"
        "$2" >"$1/logs/window.stdout.log" 2>"$1/logs/window.stderr.log" & child=$!
        trap '\''kill "$child" "$wm" 2>/dev/null || true; wait "$child" "$wm" 2>/dev/null || true; exit 143'\'' TERM INT
        wait "$child"
      ' _ "$dir" "$DESKTOP" >"$dir/logs/session.log" 2>&1 & apppid=$!
    if [[ $SEND_AT != 0 ]]; then
      (
        sleep "$SEND_AT"
        xdo mousemove 600 750 click 1 type --clearmodifiers --delay 10 'hello benchmark'
        date +%s.%N >"$dir/logs/send.txt"
        xdo key Return
      ) & actionpid=$!
    elif [[ $OPEN_AT != 0 ]]; then
      (
        sleep "$OPEN_AT"
        date +%s.%N >"$dir/logs/open.txt"
        xdo mousemove 180 400 click 1
      ) & actionpid=$!
    fi
    wait "$recpid"; recpid=
    if [[ -n $actionpid ]]; then kill -TERM "$actionpid" 2>/dev/null || true; wait "$actionpid" 2>/dev/null || true; actionpid=; fi
    pactl list short sink-inputs >"$dir/logs/sink-inputs-after.log" 2>&1 || true
    # Ask this display's window to close normally, then terminate only its process group if needed.
    window=$(xdo search --onlyvisible --name '^HoH$' | head -1 || true)
    if [[ -n $window ]]; then xdo windowactivate --sync "$window" key --clearmodifiers Alt+F4 || true; fi
    for _ in {1..30}; do kill -0 "$apppid" 2>/dev/null || break; sleep .1; done
    kill -TERM -- "-$apppid" 2>/dev/null || true
    wait "$apppid" 2>/dev/null || true
    apppid=
    if [[ $RECORD == 1 ]]; then ffprobe -v error -select_streams v:0 -show_entries frame=pts_time -of csv=p=0 "$dir/capture.mkv" >"$dir/logs/frame-times.txt"; fi
    if [[ $RESTART == 1 ]]; then
      if [[ $RESTART_PENDING == 1 ]]; then
        python3 - "$dir/data/local.hoh.desktop/localstorage/tauri_localhost_0.localstorage" <<'PENDING'
import json, sqlite3, sys
c=sqlite3.connect(sys.argv[1]); row=c.execute('SELECT value FROM ItemTable WHERE key=?',('t3code:client-settings:v1',)).fetchone()
data=json.loads(row[0].decode('utf-16le')); data['onboardingCompletedAt']=None
c.execute('UPDATE ItemTable SET value=? WHERE key=?',(json.dumps(data).encode('utf-16le'),'t3code:client-settings:v1')); c.commit(); c.close()
PENDING
      fi
      pactl list short sink-inputs >"$dir/logs/restart-sink-inputs.log" 2>&1 || true
      if [[ $RECORD == 1 ]]; then
        "${isolated[@]}" ffmpeg -nostdin -loglevel info -debug_ts -use_wallclock_as_timestamps 1 -f x11grab -framerate 10 -video_size 1440x900 -i "$display" -t "$CAPTURE_SECONDS" -c:v ffv1 "$dir/restart.mkv" >"$dir/logs/restart-capture.log" 2>&1 & recpid=$!
      else
        sleep "$CAPTURE_SECONDS" & recpid=$!
      fi
      sleep 1
    setsid env -u DBUS_SESSION_BUS_ADDRESS DISPLAY="$display" HOME="$dir/home" CLAUDE_CONFIG_DIR="$dir/home/claude-config" CODEX_HOME="$dir/home/codex-config" XDG_CONFIG_HOME="$dir/config" XDG_CACHE_HOME="$dir/cache" XDG_DATA_HOME="$dir/data" XDG_STATE_HOME="$dir/state" XDG_RUNTIME_DIR="$dir/runtime" DSH_AGENTS_HOME="$dir/agents" HOH_ENGINE_PATH="$dir/bin/engine-proxy" U5_PROXY_LOG="$dir/logs/restart-engine-frames.jsonl" PATH="$dir/bin:$PATH" GDK_BACKEND=x11 GIO_USE_VFS=local LIBGL_ALWAYS_SOFTWARE=1 WEBKIT_DISABLE_COMPOSITING_MODE=1 PULSE_SERVER=unix:/nonexistent PIPEWIRE_REMOTE=/nonexistent ALSA_CONFIG_PATH=/dev/null GTK_USE_PORTAL=0 NO_AT_BRIDGE=1 \
      dbus-run-session --config-file="$dir/runtime/bus.conf" -- bash -c '
        metacity >"$1/logs/restart-metacity.log" 2>&1 & wm=$!
        trap '\''kill "$wm" 2>/dev/null || true; wait "$wm" 2>/dev/null || true'\'' EXIT
        date +%s.%N >"$1/logs/restart-process-start.txt"
        "$2" >"$1/logs/restart-window.stdout.log" 2>"$1/logs/restart-window.stderr.log" & child=$!
        trap '\''kill "$child" "$wm" 2>/dev/null || true; wait "$child" "$wm" 2>/dev/null || true; exit 143'\'' TERM INT
        wait "$child"
      ' _ "$dir" "$DESKTOP" >"$dir/logs/restart-session.log" 2>&1 & apppid=$!
      wait "$recpid"; recpid=
      kill -TERM -- "-$apppid" 2>/dev/null || true
      wait "$apppid" 2>/dev/null || true
      apppid=
      if [[ $RECORD == 1 ]]; then ffprobe -v error -select_streams v:0 -show_entries frame=pts_time -of csv=p=0 "$dir/restart.mkv" >"$dir/logs/restart-frame-times.txt"; fi
    fi
    printf 'N=%s run=%s captured: %s\n' "$count" "$run" "$dir"
  done
done
printf 'Artifacts: %s\n' "$OUT"
