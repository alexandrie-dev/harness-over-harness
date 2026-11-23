# T3 window locator contract

The release window is WebKitGTK inside Tauri. The test runner addresses the
rendered accessibility tree by role, accessible name, text, state and geometry;
it never selects a CSS class. `ui/scripts/qa-window-formula.sh` established the
project's real-window path (`Xvfb` + `xdotool` + screenshot). Official Tauri
WebDriver would additionally require `tauri-driver` and `WebKitWebDriver`; neither
is installed on this QA host, the supplied release has no embedded WebDriver
plugin, and this task forbids installing or rebuilding anything.

## Runtime locators

| Surface | Locator |
| --- | --- |
| first run | landmark `Welcome`, heading `Welcome to HoH`, button `Start` |
| post-Welcome choice | button `Start without a project` |
| sidebar | landmark `Threads`; buttons `New chat`, `Learning`, `Widgets` |
| empty home | heading matching `Good (morning|afternoon|evening)` |
| composer | entry whose name begins `Message`; button `Submit message` |
| model | combobox whose name begins `Claude`; picker text `Effort`/`Reasoning` |
| running turn | button `Stop generation` |
| recent chat | button name `<thread title>, No project` inside `Threads` |
| conversation | landmark `Conversation`; heading matching the thread title |
| permission | buttons `Approve`/`Decline`; card accessible name containing `Waiting for your answer`; visible `Needs your answer` and command in PNG |
| placeholder pages | sidebar button plus page heading; exact `Coming soon` source contract and PNG |

The contenteditable composer exposes the textbox role but not AT-SPI's
`EditableText` interface. The runner focuses it through `Component.grab_focus()`
and sends keyboard input with `xdotool`; finding and activation remain semantic.

## HoH insertion attributes

WebKitGTK does not expose arbitrary `data-*` values through AT-SPI. These are
therefore source-audited contracts and map to the runtime semantics above:

| Attribute | Source | Runtime equivalent |
| --- | --- | --- |
| `data-hoh-home-body` | `ui/src/t3-entry/hoh-screens/HomeBody.tsx` | empty home greeting above the real composer |
| `data-hoh-page-item="learning|widgets"` | `SidebarTop.tsx` | buttons `Learning` / `Widgets` |
| `data-hoh-page="learning|widgets"` | `SidebarPages.tsx` | visible heading and `Coming soon` |
| `data-approval-detail="complete"` | `HomeBody.tsx` | human-readable command beside `Needs your answer` |
| `data-hoh-emblem` | `ui/src/t3-entry/emblem-fit.ts` | visual Alexandrie notch/emblem evidence |

Visual-only claims—Alexandrie pseudo-elements, card arrangement and overlap at
800×550—are backed by PNGs and manual inspection recorded in `REPORT.md`.
PNGs capture the Xvfb root framebuffer and crop it to the AT-SPI `HoH` frame;
capturing WebKitGTK's individual X11 surface can return a blank backing store.
