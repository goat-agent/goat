# AGENTS.md — goat-api

The method surface: `Method` contracts with per-method versions, a `Grant`, a `Direction` and a
`Shape`. `BuildId` and `Busy` live here rather than in `goat-daemon` because the client reads them.

## Adding a method

Add a line to `registry()`. A route served without that line is invisible to both generated files,
which `goat-daemon`'s `every_served_method_is_a_frozen_contract` refuses.

## Two frozen artifacts

| File | What it is |
|---|---|
| `methods_fingerprint.txt` | every contract frozen; changing a param type without bumping its version fails CI |
| `methods_schema.json` | the same table as full JSON Schema |

The fingerprint is what makes "hash the envelope, not the payload" safe; see
`crates/goat-wire/AGENTS.md`.

`methods_schema.json` is **the artifact non-Rust clients generate from**. The wire is
length-delimited JSON, so a Mac app or a Chrome panel needs no Rust, no FFI and no sidecar: read a
four-byte length, decode JSON.

Both files are generated and frozen by a test. Regenerate with:

```
cargo run -p goat-api --bin methods_schema crates/goat-api/src/methods_schema.json
```

## The `Router` is built from grants, not from the transport

A `Router` without `Grant::Admin` does not contain the admin routes at all, so a peer calling one gets
`unknown_method`. That beats a check someone can forget.
