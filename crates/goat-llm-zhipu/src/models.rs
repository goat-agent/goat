use goat_llm::ModelInfo;

inventory::submit! { ModelInfo { provider: crate::ID, id: "glm-5",       context: 200_000 } }
inventory::submit! { ModelInfo { provider: crate::ID, id: "glm-4.6",     context: 128_000 } }
inventory::submit! { ModelInfo { provider: crate::ID, id: "glm-4-flash", context: 128_000 } }
