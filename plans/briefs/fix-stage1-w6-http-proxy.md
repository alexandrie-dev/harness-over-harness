# Fix round for `hoh-http-proxy` after the lead's run and an independent review

Your port is accepted as a base: the lead ran all four suites outside the sandbox (98 passed, the loopback
listeners work there), and the reviewer found the policy identical to the source — variables, precedence,
the matcher, the three diagnostic texts. What follows is about the transport and the tests. Same boundaries
and commands as `plans/briefs/stage1-writer.md`; environment
`CARGO_NET_OFFLINE=true CARGO_BUILD_JOBS=4 CARGO_TARGET_DIR=/mnt/toshiba/alexandrie/hoh/target-w6`.
Tests that open loopback listeners cannot run in your sandbox: write them, run what runs, and say which you
could not run — the lead runs them.

## 1. The client must behave as the source's transport does when nothing is said

`src/install.rs` sets only `no_proxy`, `http1_only` and `proxy`, so reqwest's defaults apply: no connect
timeout, no read timeout, 10 redirects, a `Referer` header on redirects. The source's requests go through
Node's `fetch` over undici, whose defaults are different (read from the undici bundled in Node 22:
`timeout == null ? 1e4`, `headersTimeout != null ? headersTimeout : 3e5`, the same for `bodyTimeout`,
`redirectCount === 20`, `referrer: "no-referrer"`). Decided, in the one place that builds every client:

- `connect_timeout` 10 s;
- `read_timeout` 300 s (reqwest's per-read timeout stands for both the header and the body timeout);
- `redirect(Policy::limited(20))`;
- `referer(false)`.

Tests: the settings are visible on what `route_client_builder` returns; one test with a listener that
accepts and never answers would take 300 s, so do not write it.

`PORT_NOTES.md`, for consumers: a consumer that must not follow redirects (the source's web fetch passes
`redirect: 'manual'`) builds its own client from `route_client_builder` with `Policy::none()`; Node's `fetch`
adds request headers that reqwest does not — `accept: */*`, `accept-language: *`, `sec-fetch-mode: cors`,
`user-agent: node`, `accept-encoding: gzip, deflate` (`br, gzip, deflate` for https) — and decompresses the
answer; whoever needs a request to look the same on the wire sets them, and decompression is decided with
the first consumer of bodies (`docs/porting/DEPS.md` 5.5, decision 1).

## 2. A variable that is not valid Unicode is read as unset

`src/install.rs`: `std::env::var(name).ok()`. Node gives the source a string for any value
(`process.env[name]`, `install.ts`), so the name counts as set; the port treats it as unset and
`proxy_environment_for_child` hands the child the resolved value instead of the user's. Read with
`std::env::var_os` and `to_string_lossy`, as `crates/hoh-launch-environment` does. A test through whatever
seam the crate has for the environment (do not set variables of the test process).

## 3. Reqwest may go direct when it cannot use the proxy URL

`reqwest::Proxy::custom` drops a URL it cannot convert (`result.ok()` in `proxy.rs`; hyper-util's
`parse_env_uri` returns `None` for what `http::Uri` rejects). The policy would say "proxied" while the
request leaves directly — for a proxy that exists to keep traffic inside, the worst failure there is.
Establish from the two crates' sources (`~/.cargo/registry/src`) whether any URL the policy accepts
(`url::Url`) can be refused there. Either way make it impossible: when the installation or a route is built,
check that each proxy URL converts the way reqwest will convert it, and report a URL that does not exactly
as the policy reports an invalid proxy URL. Tests: such a URL if one exists; otherwise the reason in the notes.

## 4. `Debug` prints the proxy password

`ProxyInstallation`, `ProxyRoute`, `ProxyPolicy` derive `Debug` and hold raw proxy URLs and the inherited
environment. The diagnostic texts are clean, but `{:?}` is what a Rust log or a panic message prints. The
`Debug` output of these types shows a proxy URL without its password (`http://user:***@host:3128`), in the
URL fields and in the environment values of the proxy variables. A test that `format!("{:?}", …)` of each
type does not contain the password. `Display`, serialisation and the values handed to a child are unchanged.

## 5. Tests that prove what the source's tests prove

- `tests/install-http1.rs`: `assert_h2` formats a builder made afresh by the test; no case looks at the
  installation or the route that the code under test built, `keeps_http2_enabled_when_the_caller_opts_in`
  cannot tell `Some(true)` from `None`, and `response.version() == HTTP_11` over plain HTTP is true with or
  without the pin. The source asserts on the `Agent`, `ProxyAgent` and `Pool` actually built
  (`install-http1.spec.ts`). Make the decision observable on the built object (the installation and each
  route record whether HTTP/1.1 is pinned, and the client is built from that record in one place), and
  assert it there for every case, including the route layered over an installed proxy.
- `tests/install.rs`, `carries_the_dispatcher_already_routing…`: the source asserts identity
  (`expect(route.dispatcher).toBe(getGlobalDispatcher())`) — one transport, not a copy. A `reqwest::Client`
  has no identity to compare; prove the same thing by behaviour: two requests, one through the
  installation's client and one through the carried one, reuse a single connection of a keep-alive listener
  (the listener accepted once).
- The tests about disposal (`restores_the_dispatcher_the_route_and_the_environment_on_disposal`,
  `removes_an_environment_name…`): the port has no global state to restore, so they check that layering
  leaves the outer installation's data unchanged. Say that in a comment on each and in the notes' table of
  ported cases.

## 6. Notes

`PORT_NOTES.md`, section on differences (today TLS is only under "Не проверено"):
- trust roots: reqwest with `rustls` verifies through the operating system's store
  (`rustls-platform-verifier`), where Node ships its own roots; a corporate root installed in the system is
  trusted without configuration; `NODE_EXTRA_CA_CERTS`, the remedy the source's README gives, is not read
  (`SSL_CERT_FILE` and `SSL_CERT_DIR` are, by the verifier, from the process environment);
- on a host with no certificate store at all `build()` fails (`reqwest` `client.rs`, `Verifier::new`), so
  `install_proxy_from_environment` returns an error even for a direct policy, where the source cannot fail:
  an image HoH is shipped in must contain the system certificates. Check the claim in the verifier's source
  and correct this sentence if it is not so.

## Done when

`cargo test -p hoh-http-proxy --no-fail-fast` passes as far as your sandbox allows (name the tests that need
listeners), `cargo clippy -p hoh-http-proxy --all-targets -- -D warnings` and
`cargo fmt -p hoh-http-proxy -- --check` are clean. Write each new test before its fix where the sandbox can
run it. If you disagree with an item, leave the code and say why with the `file:line` of the source or of
the library.

Your last message is the report: the result line of each command, one line per item, the tests you could
not run. At most 300 words.
