use goat_llm::ModelInfo;

inventory::submit! { ModelInfo { provider: crate::ID, id: "gemini-3-pro",     context: 1_000_000 } }
inventory::submit! { ModelInfo { provider: crate::ID, id: "gemini-3-flash",   context: 1_000_000 } }
inventory::submit! { ModelInfo { provider: crate::ID, id: "gemini-2.5-pro",   context: 1_000_000 } }
inventory::submit! { ModelInfo { provider: crate::ID, id: "gemini-2.5-flash", context: 1_000_000 } }
