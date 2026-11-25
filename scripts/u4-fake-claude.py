#!/usr/bin/python3 -I
"""Argv-checking Claude stand-in; delegates protocol to the explicitly supplied test stub."""
import json
import os
import sys

args = sys.argv[1:]
if '--version' not in args:
    with open(os.environ['U4_ARGV_LOG'], 'a', encoding='utf-8') as log:
        log.write(json.dumps(args) + '\n')
    try:
        sources = args[args.index('--setting-sources') + 1].split(',')
        if 'user' in sources:
            settings = json.loads(args[args.index('--settings') + 1])
            assert settings['enabledPlugins']['telegram@claude-plugins-official'] is False
    except (ValueError, IndexError, KeyError, AssertionError):
        print('missing official Telegram disable overlay', file=sys.stderr)
        sys.exit(2)
    if '--json-schema' in args and '--no-session-persistence' in args:
        print('unknown option --no-session-persistence', file=sys.stderr)
        sys.exit(2)
os.execv(os.environ['U4_STUB'], [os.environ['U4_STUB'], *args])
