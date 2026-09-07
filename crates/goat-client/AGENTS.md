# AGENTS.md — goat-client

Transport-agnostic. `Link::{Local,Remote}` dials either a unix socket or mTLS+WebSocket and hands back
the same `Api`, so nothing above it knows which it got.

Local-daemon autostart lives behind `Link::dial_or_spawn` and fires only for `Link::Local`. A remote
target that cannot connect must fail, never silently start a second daemon here.

## The admin pump

A slash command reaches the daemon through `CommandEffect::Admin(Vec<AdminRequest>)`. It is a batch,
so `/search` can write a key and its config rows in one order.

`Attachment` carries `admin: Sender<AdminRequest>` beside its `Op` sender. This crate's pump turns an
`AdminRequest::ProviderLogin` into a local OAuth run plus one `admin.credential_set`.

A remote link refuses `ProviderLogin` outright, because a remote daemon reads its own credentials.

`goat-tui` and `goat-command-*` name credential keys but never construct a `CredentialStore`, so a
client-side write is unrepresentable rather than merely absent. See `crates/goat-auth/AGENTS.md`.
