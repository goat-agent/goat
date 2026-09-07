# AGENTS.md — goat-auth

Credential storage (`CredentialStore`, `CredentialKey`, `CredentialValue`) and the OAuth loopback.

## Credentials cross one door

`admin.credential_set` and `admin.credential_remove` take a `CredentialKey` plus a `CredentialValue`.
That is the same `#[serde(tag = "kind")]` encoding `credentials.json` uses on disk, reused rather
than mirrored so the two cannot drift.

`Credential` itself stays unserializable. That is the guardrail keeping a secret out of a log line.

**Acquisition is the client's, storage is the daemon's.** OAuth loopback, prompts and device flows run
where the human is, and only the acquired credential crosses. A write fans `Op::RefreshAccounts {}`
out to every live session, which is why `goat provider login` in one terminal reaches a running
`goat code` in another.

Store a credential that failed validation and report `verification_failed`. A network blip must not
delete a key.

## The redirect is parsed here and validated elsewhere

The loopback capture returns the whole `AuthorizationResponse` — `code` plus the RFC 9207 `iss` — and
checks only `state`, the one value it issued itself. It holds no metadata, so it cannot judge the
issuer. `goat-mcp` does the discovery, passes `iss` to rmcp, and lets rmcp compare.

**Never add issuer validation here, and never let a capture helper return just a code.** Dropping the
rest of the response is what broke Sentry login.

## `bind_loopback`

It binds port 0 and cannot fail; `goat-provider-gemini` and `goat-provider-anthropic` depend on that.
A caller needing fixed ports gets a new function beside it, not a change to it. Owning a fixed port
is already the idiom for such callers (`goat-provider-openai-codex`, `goat-provider-xai`).
