# AGENTS.md — goat-wire

One transport surface, and there is no second one.

| Module | Owns |
|---|---|
| `envelope.rs` | six frame kinds — `hello`, `req`, `res`, `data`, `end`, `cancel` — with opaque JSON payloads |
| `codec.rs` | length-delimited JSON framing |
| `transport.rs` | the unix socket |
| `peer.rs` | the duplex state machine |

## Keep engine vocabulary out of the envelope

Payloads stay opaque, so `envelope_fingerprint` moves only when the envelope itself changes. A test
asserts the envelope schema never mentions engine vocabulary.

The payload is still typed, one level up: `goat-api`'s per-method contracts carry that weight.

## `peer.rs`

One reader task, which never awaits a handler.

Three outbound lanes in priority order — **control > data > requests** — so a flooding stream cannot
starve a response.

The id space splits by parity: **odd is client-originated, even is daemon-originated.**

## Type placement

Types that never leave the daemon go in `goat-daemon`'s `wire.rs`. `BuildId` and `Busy`, which the
client also reads, go in `goat-api`.
