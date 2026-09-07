# AGENTS.md — goat-sandbox

The Seatbelt profile wrapped around every shell call. The surface is two functions:
`backend_available()` and `read_only_command(command, cwd, writable_tmp, network)`.

## Read-only, and macOS-only

There is one constructor, and it builds a read-only profile: `(deny default)` plus explicit allows.

| | |
|---|---|
| `file-write*` | only the passed tmp path, `/private/tmp`, `/private/var/tmp`, and a fixed list of `/dev` nodes |
| network | a parameter, not a default |
| secret directories | `.ssh`, `.goat`, `.aws`, `.gnupg`, `.config/gcloud`, … denied explicitly, even though reads are otherwise allowed |

On a platform with no backend, `backend_available()` is false and `read_only_command` returns
`SandboxError::Unavailable`. The caller decides the fallback; this crate never silently runs
unsandboxed.

## Why it matters beyond this crate

This profile is why desktop automation may not drive Terminal.app: doing so would run shell commands
outside it. See `crates/goat-capability/AGENTS.md`.
