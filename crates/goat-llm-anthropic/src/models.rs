use goat_llm::ModelInfo;

inventory::submit! { ModelInfo { provider: crate::ID, id: "claude-opus-4-7",   context: 200_000 } }
inventory::submit! { ModelInfo { provider: crate::ID, id: "claude-sonnet-4-6", context: 200_000 } }
inventory::submit! { ModelInfo { provider: crate::ID, id: "claude-haiku-4-5",  context: 200_000 } }
