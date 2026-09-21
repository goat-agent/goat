# AGENTS.md — goat-provider-builtin

The product's provider table: fourteen `Row` consts in `rows.rs` over the
`goat-provider-openai-compat` wire base, plus the spec merge that turns `Row` + user patch into a
`ProviderSpec`.

## `Row` is the builtin definition, `ProviderSpec` is the effective one

`Row` keeps what a file cannot express: `model_filter`/`vision_filter`/`efforts`/`effort_wire` fn
pointers, `catalog_only`/`live_model_list`/`stream_options`/`reasoning_effort` flags, and
`ProviderMetadata`. `base_spec(row)` converts the data half into a `ProviderSpec`;
`spec_for(row, patch, store, account)` merges the user's `[providers.<id>]` patch on top and
resolves the endpoint (`credential.endpoint` > `spec.endpoint` > `row.base_url`).

`build_openai_spec(row, spec, store, account)` is the only constructor for the `chat`/`responses`
dialects — used for patched `Row`s (fn fields come from the `Row`) and for customs (`row = None`,
dialect defaults). Other dialects dispatch to their own crates' `build_connected`.

## Validators

- `validate_id` — custom provider ids: 1-39 chars, `[a-z0-9][a-z0-9-]*`, not a builtin alias.
- Patch endpoints on auth-bearing rows: `validate_override_endpoint` (https or loopback). On
  `AuthMethod::None` rows: `validate_user_endpoint` (http allowed — no credential flows).
- Credential endpoints (`ApiKeyWithEndpoint`): the row's `endpoint_override.validate` — qwen keeps
  its Alibaba allowlist; hosted rows get https-or-loopback; local rows get the http-tolerant one.
- `is_custom` reads `metadata().validation == "custom"`; `custom_metadata` stamps it.

## Adding a `Row`

`Row::hosted(id, base_url, host, env_var)` for a hosted API-key provider; `Row::local(id, base_url)`
for a localhost one. Then one `BUILTINS` line in `goat-providers`. `host` feeds
`enforce_https_host` on the *default* base URL only — user-declared endpoints are checked by the
patch validator instead.
