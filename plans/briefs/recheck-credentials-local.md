# Closure check: hoh-credentials-local fix round

You only read. Do not run builds, tests, servers or anything that writes; do not edit files. The lead runs the tests.
Review: `/mnt/toshiba/alexandrie/hoh/logs/rv-cred-local.report.md` (6 items). Writer s9 reports all closed, 143/0, PORT_NOTES updated.
Claims: (1) unknown version/kind values never printed (boot error and reload warning), departure from TS recorded;
(2) manual Debug of the document without values; (3) inserts before `...`, delete keeps markers; (4) flow-style document is
rewritten as block only if lossless, otherwise refused with "nothing was written", file unchanged; verify compares the whole
document (other top-level keys, other records, number spelling); (5) watcher follows the nearest existing ancestor and
retargets on create/delete; (6) an existing directory with group/other access is refused with `run "chmod 700 …"`, new dirs 0700.
For each item: closed / not closed, file:line, test, fails on old code? Also: can the key still appear anywhere (panic messages,
the YAML parser's own error text quoting the source line, watcher warnings with content, temp file names/contents left on
failure)? Is the lossless check sound (quoted strings, anchors/aliases, tags, multi-document)? Any race between the watcher
retarget and a concurrent write? New defects. ≤500 words, Russian.
