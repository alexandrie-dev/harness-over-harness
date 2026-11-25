# Draft: session broker for Rust plugins

This is an interface proposal, not an implemented authorization boundary. In-process Rust plugins have the privileges of the HoH process; grants constrain calls through this broker and do not sandbox native code.

A future `SessionBroker` is obtained for a mounted plugin and an explicit session scope. Every operation checks the current grant at the point of use, including after a plugin is disabled or its grants change. A missing or revoked grant returns `PermissionDenied` before accessing session content or performing side effects. The caller cannot substitute another plugin's identity.

| Operation | Required manifest permission | Result |
| --- | --- | --- |
| `create_chat(options)` | `sessions.create` | A session handle in the permitted scope. |
| `send_message(session, message)` | `sessions.send` | The started turn identifier; reject if the session is outside scope. |
| `wait_turn(session, turn)` | `sessions.wait` | Terminal outcome; cancellation of the wait does not stop the turn. |
| `stop(session, turn)` | `sessions.stop` | Acknowledgement of the stop request, not an assertion that the turn has ended. |
| `read_events(session, cursor)` | `sessions.events.read` | A bounded page and a continuation cursor; conversation content requires this explicit grant. |

The host must validate session ownership and scope separately from each action grant. Event subscriptions must be bounded, disclose dropped events, and terminate when the plugin is disposed. Calls must fail closed when permission state cannot be resolved. `wait_turn` must handle turns already finished before the call and the race between observing state and subscribing. Stop and terminal events must remain idempotent under retries. Error responses must not reveal conversation content or the existence of inaccessible sessions.

Before exposing the interface, test every operation without its respective grant, revocation while waiting, access to an out-of-scope session, concurrent stop/completion, and subscription cleanup on unload. Limits, persistence, and cross-session delegation require separate design.
