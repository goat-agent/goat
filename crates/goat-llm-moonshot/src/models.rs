use goat_llm::ModelInfo;

inventory::submit! { ModelInfo { provider: crate::ID, id: "kimi-k2.6",        context: 256_000 } }
inventory::submit! { ModelInfo { provider: crate::ID, id: "kimi-k2.5",        context: 128_000 } }
inventory::submit! { ModelInfo { provider: crate::ID, id: "kimi-k2-thinking", context: 128_000 } }
