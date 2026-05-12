use goat_llm::ModelInfo;

inventory::submit! { ModelInfo { provider: crate::ID, id: "gpt-5",       context: 400_000 } }
inventory::submit! { ModelInfo { provider: crate::ID, id: "gpt-5-mini",  context: 400_000 } }
inventory::submit! { ModelInfo { provider: crate::ID, id: "gpt-4o",      context: 128_000 } }
inventory::submit! { ModelInfo { provider: crate::ID, id: "gpt-4o-mini", context: 128_000 } }
inventory::submit! { ModelInfo { provider: crate::ID, id: "o3",          context: 200_000 } }
