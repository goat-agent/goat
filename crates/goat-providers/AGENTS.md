# AGENTS.md — goat-providers

The LLM provider registry. `Registry::load_metered` is one ordered list mixing
`goat_provider_builtin::build(&rows::…)` calls with the five code-provider crates. Identity is a
runtime `ProviderId::from("…")`.

**No `inventory` here**, unlike channels, integrations and agent tools.

## Adding a provider

A data-only provider is a `Row` const in `goat-provider-builtin` plus one registry line. Write a
`goat-provider-<name>` crate only when the provider needs code:

| Reason | Crates |
|---|---|
| its own wire format | anthropic, gemini |
| OAuth flow or runtime headers | openai-codex, kimi-code |
| credential-kind dispatch | xai |

`goat-provider-openai-compat` registers nothing; it is the chat/Responses wire base.
`goat-provider-builtin` is the product's table over it: eleven hosted providers plus the local trio
(ollama, lmstudio, llama-cpp).

Keep provider-specific request bodies, streaming, auth and error mapping inside each provider crate.
Do not add shared "quirks" flags. Provider names may appear in exactly two places: the provider table
and `Registry::load_metered`.

## Fingerprint

This crate's fingerprint test freezes the registry's observable surface. After a deliberate provider
change, regenerate:

```
cargo test -p goat-providers fingerprint::regenerate -- --ignored
```

## User-declared providers

The `providers` map in `config.json` is written by `goat provider add`/`remove`, never by hand. Those
entries join the same pipeline at load time as OpenAI-compatible chat providers with live `/models`
discovery, and their keys stay in `credentials.json`.

`Registry` reads them through the `UserProviders` handle, constructor-injected like `CredentialStore`
and re-read on every registry build.

`goat-search-providers::metadata` stays a hardcoded list. Search backends live in
`goat-search-provider-<name>`.
