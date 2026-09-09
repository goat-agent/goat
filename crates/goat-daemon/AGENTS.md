# AGENTS.md — goat-daemon

One daemon serving the local socket and remote clients through the same `serve_envelope`.
`CodeSessionHub` lives here, not in an engine crate.

## Boot order

`goat daemon serve` runs these in order under one shutdown token:

1. take `~/.goat/daemon.lock`;
2. build one `CodeSessionHub`;
3. **bind the socket and start accepting**;
4. open `ProxyStore`, spawn a `Recorder` with two `Meter`s (one code, one agent);
5. boot the agent runtime via `AgentRuntime::boot_with_code_metered`;
6. backfill `rate_limits.json` and serve the proxy dashboard.

Keep step 3 ahead of everything that touches the network, or every client wait budget becomes a lie.
`daemon.status` reports `ready: false` until the agent runtime is up; code sessions never wait on it.

The `code_task` tool drives that same `CodeSessionHub` in-process, with no wire hop.

## The lock is the handoff barrier, not the socket

`transport::cleanup` unlinks the socket while the runtime still has up to 10 s of drain left. "Socket
gone" therefore does not mean "process gone".

The kernel releases an `flock` on process death, SIGKILL included, so a replacement daemon blocks on
`acquire` until the old one is really out. That is also what makes the global boot sweeps
(`WHERE status = 'running'`) safe.

## Detaching is a spawn contract

`goat daemon serve` stays in the foreground so a supervisor can own it. `goat daemon start` spawns it
with the hidden `--detached`, which then calls `rustix::process::setsid()`.

Never decide this from job control instead: in a non-interactive shell the process is not a group
leader and would detach from its supervisor.

## The daemon greets first, and nothing gates a version

On accept it sends one `Hello` carrying every `(method, versions)` its router holds, its grant set,
and an `info` object (`build`, `epoch`, `pid`, `client_id`). `Api::negotiated` picks a version per
method from it.

Skew is therefore per method and typed. Calling something the daemon does not serve fails with
`ErrorCode::UnsupportedVersion` before the call leaves the client, rather than as one
connection-wide compatible/incompatible verdict.

Keep `build` inside `info` rather than promoting it to a `Hello` field, so `envelope_fingerprint`
does not move whenever a deployment fact is added. It decides nothing and only feeds a human-readable
line.

Leave destructive judgment with the state's owner: `admin.daemon_stop { if_idle }` lets the daemon
refuse while a turn is running. A client reading a connect-time snapshot could not do that without
racing a `cron` turn. `goat daemon stop` waits for EOF, which the daemon sends only after a full
drain.

## The subscriber bus is `session::Update`, not a wire type

Four variants: snapshot, event, presence, error. `api::watch_item` stamps each with a cursor to make a
`WatchItem`, and that mapping is total — every update reaches the client, including the error a
stopping engine emits.

`build_snapshot` produces a `goat_api::SessionSnapshot` directly, so nothing translates between an
internal shape and the published one.

## Sessions

Keyed by `SessionId`, with a secondary index by `conversation_id`. **There is no cwd map.**
`goat code` opens a new session by default, and only `-c` resolves cwd to the latest conversation
through a database query. Several live sessions can share a cwd.

Canonicalize the working directory before constructing the engine. Persistence and conversation
lookup must use that same path, including aliases such as macOS `/tmp` and `/private/tmp`.

## Tests

`tests/` exists because these need a real socket: `roundtrip`/`lifecycle` bind one; `remote_e2e` adds
mTLS and drives the real `goat_remote::client` rather than a hand-rolled TLS helper, covering both
halves at once; `browser_capability` drives a `BrowserPort` through the broker.

All are pinned to `max-threads = 1` in `.config/nextest.toml`. Add a new socket-binding test to that
filter.
