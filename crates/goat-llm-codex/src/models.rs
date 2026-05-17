use goat_llm::ModelInfo;

// Verified live against `GET https://chatgpt.com/backend-api/codex/models`.
// ChatGPT-account backend exposes slugs without the `-codex` suffix; the
// server may transparently upgrade (e.g. `gpt-5.2` → `gpt-5.4`).
inventory::submit! { ModelInfo { provider: crate::ID, id: "gpt-5.2", context: 272_000 } }
