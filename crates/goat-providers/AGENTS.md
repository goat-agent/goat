# AGENTS.md — goat-providers

The LLM provider registry. `Registry::load_metered` walks one ordered `BUILTINS` table — `Row`
entries from `goat-provider-builtin`, the `anthropic`/`gemini` dialect crates, and the four
`Special`s (`openai-codex`, `kimi-code`, `xai`, `devin`) — then appends user-defined providers.
Identity is a runtime `ProviderId::from("…")`.

**No `inventory` here**, unlike channels, integrations and agent tools.

## Specs: builtin patch and dialect reuse are one mechanism

`~/.goat/config.toml` `[providers.<id>]` entries are `ProviderSpecConfig` (defined in
`goat-provider`). The id decides what an entry is:

- **builtin id** → a *patch* merged over the builtin's `ProviderSpec` (scalars replace, map fields
  merge per-key, `catalog` replaces). Patchable = the 14 `Row`s + `anthropic` + `gemini`.
- **new id** → a *custom spec*; `wire` picks the dialect (`chat` default, `responses`, `anthropic`,
  `gemini`) and `endpoint` is required.
- **Special id** → only `disabled`/`name` are honored; other fields make the entry `invalid`.

Endpoint resolution is `credential.endpoint` (`ApiKeyWithEndpoint`, per-account) > `spec.endpoint`
> builtin default. A foreign effective endpoint flips `model_list_source` to `Discover` and turns
`web_search`/`tool_search` off unless `features` opts back in.

Invalid entries never kill the build: they land in `Registry::invalid()` (surfaced by
`provider list`/doctor) while the rest load — builtins fall back to unpatched, customs are skipped.
`validate_spec_entry` is the same check applied at `admin.config_edit` write time.

## Adding a provider

A data-only provider is a `Row` const in `goat-provider-builtin` plus one `BUILTINS` line. Write a
`goat-provider-<name>` crate only when the provider needs code:

| Reason | Crates |
|---|---|
| its own wire format | anthropic, gemini |
| OAuth flow or runtime headers | openai-codex, kimi-code |
| credential-kind dispatch | xai |
| non-HTTP protocol | devin |

`goat-provider-openai-compat` registers nothing; it is the chat/Responses wire base.
`goat-provider-builtin` is the product's table over it plus the spec merge (`spec_for`,
`build_openai_spec`).

Keep provider-specific request bodies, streaming, auth and error mapping inside each provider crate.
Do not add shared "quirks" flags. Provider names may appear in exactly two places: the provider table
and `BUILTINS`.

## Fingerprint

This crate's fingerprint test freezes the registry's observable surface. After a deliberate provider
change, regenerate:

```
cargo nextest run -p goat-providers --run-ignored all fingerprint::regenerate
```

`goat-search-providers::metadata` stays a hardcoded list. Search backends live in
`goat-search-provider-<name>`.
