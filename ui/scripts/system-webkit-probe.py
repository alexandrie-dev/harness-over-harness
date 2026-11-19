#!/usr/bin/env python3
"""Independent system-WebKit check. Not the Tauri-window acceptance check.
Uses only system GI; run through run-isolated-window.py.
"""
import gi
import json
from pathlib import Path
import sys

gi.require_version('Gtk', '3.0')
gi.require_version('WebKit2', '4.1')
from gi.repository import Gtk, WebKit2, GLib

window = Gtk.Window(title='HoH — CSS probe')
window.set_default_size(1200, 800)
view = WebKit2.WebView()
view.get_settings().set_allow_file_access_from_file_urls(True)
window.add(view)
state = {'result': None}


def title_changed(view, *_):
    title = view.get_title() or ''
    if title.startswith('HOH_PROBE:'):
        state['result'] = json.loads(title[len('HOH_PROBE:'):])
        print(json.dumps(state['result'], ensure_ascii=False), flush=True)
        Gtk.main_quit()


view.connect('notify::title', title_changed)
window.connect('destroy', lambda *_: Gtk.main_quit())
version = '.'.join(str(fn()) for fn in [WebKit2.get_major_version, WebKit2.get_minor_version, WebKit2.get_micro_version])
page = Path(__file__).resolve().parents[2] / 'apps/hoh-desktop/probe-dist/probe.html'
view.load_uri(page.as_uri() + '?webkitgtk=' + version)
window.show_all()
GLib.timeout_add_seconds(45, lambda: (Gtk.main_quit(), False)[1])
Gtk.main()
sys.exit(0 if state['result'] and state['result']['pass'] else 2)
