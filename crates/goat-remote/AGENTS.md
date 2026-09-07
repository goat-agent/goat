# AGENTS.md — goat-remote

Both halves of the mTLS surface: `server` (accept, pair, verify) and `client` (enroll, connect).
`ws::adapt` is the one WebSocket↔frame adapter, used in both directions.

Keep new transport work here rather than growing a second remote path. The daemon serves remote
clients through the same `serve_envelope` as the local socket, and that is what keeps the two from
drifting.

## `device` and `remote` point in opposite directions

| Command | Runs on | Manages |
|---|---|---|
| `goat device {add,ls,rm}` | the daemon host | who may reach it. `add` mints a one-time pairing code (3 min, `pairing`) and prints the server fingerprint |
| `goat remote {add,ls,rm,use}` | the client | which daemons *this* machine reaches |

`local` is a reserved remote naming the daemon on this machine. It is always in `goat remote ls`, and
`goat remote use local` is how remote is turned off; there is no separate enable flag.
`default_remote: None` means `local`, so local has exactly one representation.

Config keys are `devices` on the server side (bind/advertised; still accepts the old `remote` key via
serde alias) and `remotes` + `default_remote` on the client side, in `client.json`.

## The server proves its key, not its name

`ca.rs` issues a SAN-less leaf when `advertised` is empty, and skips regeneration once `server.crt`
exists. The client pins the server fingerprint (`verify::PinnedServer`) instead of validating a
hostname, so addresses can be bare IPs, Tailscale names or port-forwards without touching the
certificate.

**Never add hostname validation on top.** It would re-introduce the certificate-name problem that
pinning removes.

## Device key material is one identity

Stored in `credentials.json` under
`{service: remote, provider: <remote name>, account: "device", slot: "key"|"cert"|"ca"}`. Write and
remove the three slots together; a half-restored remote is a broken remote.

`config.json` keeps only addressing: `host`, `fingerprint`, `last_dir`.

The client reads this material, so the client owns it. That is why `goat remote` writing directly is
not an exception to the `admin.config_edit` rule.

## A remote client never sends its own cwd

`session.open` answers with the cwd the daemon normalized. The client records it as that remote's
`last_dir`, and the next `goat code --remote <name>` reuses it; `--dir` overrides.

Sending the local cwd would open a session on a path absent from the daemon host, which
`goat-tool`'s `ToolSandbox` then fails on for every call.

`-w` is refused for remote targets because worktrees are local git.
