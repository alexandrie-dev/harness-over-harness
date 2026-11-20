# Closure check: hoh-user-approval fixes + the kernel addition `waterfall_with_start` (w4)

You only read. Do not run builds, tests, servers or any command that writes; do not edit files.
Review being closed: `/mnt/toshiba/alexandrie/hoh/logs/rv-approval.full.md` (2 P2 items). Writer's report:
`/mnt/toshiba/alexandrie/hoh/logs/w4.kernel1.report.md`. The lead allowed one additive change in `crates/hoh-kernel`: a signal that
the initial waterfall pass has started, without changing existing behaviour or signatures (`crates/hoh-kernel/PORT_NOTES.md`,
section added by w4). Check: (1) both approval items closed, with file:line, and whether the new tests would fail on the old
code (the cancel-order test must not use the extra `ready.await`); (2) the kernel diff (`git diff crates/hoh-kernel`) is purely
additive: no existing function, type, ordering or error text changed; the new API has no fail-open path (a handler that never
starts, panics or is cancelled before start); the existing waterfall is untouched; (3) any new divergence. ≤500 words, Russian.
