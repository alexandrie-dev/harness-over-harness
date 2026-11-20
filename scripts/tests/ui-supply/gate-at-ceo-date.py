#!/usr/bin/env python3
"""Test-only clock for the production embedded gate; no clock override in its CLI."""
import datetime as dt
from pathlib import Path
import sys
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[3]
script = ROOT / 'scripts/ui-supply-check.sh'
source = script.read_text().partition("<<'PY'\n")[2].rsplit('\nPY\n', 1)[0]
assert source.startswith('import argparse\n')

test_day = '2026-11-18'
arguments = sys.argv[1:]
if arguments and arguments[0].startswith('--test-date='):
    test_day = arguments.pop(0).split('=', 1)[1]
instant = dt.datetime.fromisoformat(test_day + 'T18:00:00+00:00')

class FixedClock(dt.datetime):
    @classmethod
    def now(cls, tz=None):
        value = cls.fromisoformat(instant.isoformat())
        return value.astimezone(tz) if tz else value.replace(tzinfo=None)

sys.argv = [str(script), str(ROOT), *arguments]
with patch.object(dt, 'datetime', FixedClock):
    exec(compile(source, str(script), 'exec'), {'__name__': '__main__'})
