use goat_provider::{Effort, LoginEndpointMetadata, ProviderMetadata};
use goat_provider_openai_compat::{
    known_openai_compatible_vision_model, known_openai_vision_model, no_efforts, no_vision,
};

use crate::{Row, Wire};

pub const OPENAI: Row = Row {
    wire: Wire::Responses,
    catalog: &[
        "gpt-5.6",
        "gpt-5.6-sol",
        "gpt-5.6-terra",
        "gpt-5.6-luna",
        "gpt-5.5",
        "gpt-5.4",
        "gpt-5.4-mini",
        "gpt-4.1",
        "o3",
        "o4-mini",
    ],
    context_windows: &[
        ("gpt-5.6", 1_050_000),
        ("gpt-5.5", 1_050_000),
        ("gpt-5.4", 1_050_000),
        ("gpt-5", 400_000),
        ("gpt-4.1", 1_047_576),
        ("o3", 200_000),
        ("o4", 200_000),
    ],
    model_filter: Some(openai_chat_model),
    vision_filter: Some(known_openai_vision_model),
    search_model: Some("gpt-5.6-luna"),
    ..Row::hosted(
        "openai",
        "https://api.openai.com/v1",
        "api.openai.com",
        "OPENAI_API_KEY",
    )
};

pub const OPENROUTER: Row = Row {
    catalog: &[
        "anthropic/claude-opus-5",
        "anthropic/claude-sonnet-5",
        "openai/gpt-5.6",
        "google/gemini-3.6-flash",
        "z-ai/glm-5.2",
        "moonshotai/kimi-k3",
        "deepseek/deepseek-v4-pro",
        "minimax/minimax-m3",
        "qwen/qwen3-coder",
    ],
    context_windows: &[
        ("anthropic/claude-opus-5", 1_000_000),
        ("anthropic/claude-sonnet-5", 1_000_000),
        ("openai/gpt-5.6", 1_050_000),
        ("google/gemini-3.6-flash", 1_000_000),
        ("z-ai/glm-5.2", 1_000_000),
        ("moonshotai/kimi-k3", 1_000_000),
        ("deepseek/deepseek-v4", 1_000_000),
        ("minimax/minimax-m3", 1_000_000),
        ("qwen/qwen3-coder", 1_000_000),
    ],
    model_filter: Some(openrouter_chat_model),
    vision_filter: Some(gateway_vision_model),
    reasoning_effort: false,
    extra_headers: &[
        ("HTTP-Referer", "https://github.com/goat-agent/goat"),
        ("X-Title", "goat"),
    ],
    live_model_list: true,
    ..Row::hosted(
        "openrouter",
        "https://openrouter.ai/api/v1",
        "openrouter.ai",
        "OPENROUTER_API_KEY",
    )
};

pub const GROQ: Row = Row {
    catalog: &[
        "openai/gpt-oss-120b",
        "openai/gpt-oss-20b",
        "openai/gpt-oss-safeguard-20b",
        "qwen/qwen3.6-27b",
        "llama-3.3-70b-versatile",
        "llama-3.1-8b-instant",
        "groq/compound",
        "groq/compound-mini",
    ],
    context_windows: &[
        ("openai/gpt-oss", 131_072),
        ("qwen/qwen3", 131_072),
        ("llama-3.3", 131_072),
        ("llama-3.1", 131_072),
        ("groq/compound", 131_072),
    ],
    model_filter: Some(groq_chat_model),
    images: false,
    stream_options: false,
    reasoning_effort: false,
    ..Row::hosted(
        "groq",
        "https://api.groq.com/openai/v1",
        "api.groq.com",
        "GROQ_API_KEY",
    )
};

pub const DEEPSEEK: Row = Row {
    catalog: &["deepseek-v4-pro", "deepseek-v4-flash"],
    context_windows: &[
        ("deepseek-v4-pro", 1_000_000),
        ("deepseek-v4-flash", 1_000_000),
    ],
    images: false,
    reasoning_effort: false,
    ..Row::hosted(
        "deepseek",
        "https://api.deepseek.com",
        "api.deepseek.com",
        "DEEPSEEK_API_KEY",
    )
};

pub const MISTRAL: Row = Row {
    catalog: &[
        "mistral-large-latest",
        "mistral-medium-latest",
        "mistral-small-latest",
        "ministral-3-14b-latest",
        "ministral-3-8b-latest",
        "ministral-3-3b-latest",
        "codestral-latest",
    ],
    context_windows: &[
        ("mistral-large", 262_144),
        ("mistral-medium", 262_144),
        ("mistral-small", 262_144),
        ("ministral-3", 262_144),
        ("codestral", 262_144),
    ],
    vision_filter: Some(mistral_vision_model),
    efforts: Some(no_efforts),
    reasoning_effort: false,
    ..Row::hosted(
        "mistral",
        "https://api.mistral.ai/v1",
        "api.mistral.ai",
        "MISTRAL_API_KEY",
    )
};

pub const ZAI: Row = Row {
    catalog: &[
        "glm-5.2",
        "glm-5.1",
        "glm-5-turbo",
        "glm-5",
        "glm-4.7",
        "glm-4.7-flash",
        "glm-4.6",
        "glm-4.5",
        "glm-4.5-air",
        "glm-4-32b-0414-128k",
        "glm-5v-turbo",
    ],
    context_windows: &[
        ("glm-5.2", 1_000_000),
        ("glm-5.1", 204_800),
        ("glm-5v", 204_800),
        ("glm-5", 204_800),
        ("glm-4.7", 204_800),
        ("glm-4.6", 204_800),
        ("glm-4.5", 131_072),
        ("glm-4-32b", 131_072),
    ],
    vision_filter: Some(zai_vision_model),
    efforts: Some(zai_efforts),
    effort_wire: Some(zai_effort_wire),
    catalog_only: true,
    metadata: ProviderMetadata {
        env_var: Some("ZAI_API_KEY"),
        validation: "catalog-only",
        endpoint: None,
        oauth: Some("not supported by Z.AI API docs"),
        login_endpoint: None,
        setup: &[],
    },
    ..Row::hosted(
        "zai",
        "https://api.z.ai/api/paas/v4",
        "api.z.ai",
        "ZAI_API_KEY",
    )
};

pub const ZAI_CODING: Row = Row {
    catalog: &["glm-5.2", "glm-5.1", "glm-5-turbo", "glm-4.7"],
    context_windows: &[
        ("glm-5.2", 1_000_000),
        ("glm-5.1", 204_800),
        ("glm-5-turbo", 204_800),
        ("glm-4.7", 204_800),
    ],
    vision_filter: Some(no_vision),
    efforts: Some(zai_efforts),
    effort_wire: Some(zai_effort_wire),
    catalog_only: true,
    metadata: ProviderMetadata {
        env_var: Some("ZAI_CODING_API_KEY"),
        validation: "catalog-only",
        endpoint: Some("https://api.z.ai/api/coding/paas/v4"),
        oauth: Some("not OAuth; uses Z.AI Coding Plan API key"),
        login_endpoint: None,
        setup: &[
            "Z.AI Coding Plan API-key provider.",
            "Use `ZAI_CODING_API_KEY` or `goat provider login zai-coding --key sk-...`.",
            "This is not OAuth and does not reuse the standard `zai` credential.",
        ],
    },
    ..Row::hosted(
        "zai-coding",
        "https://api.z.ai/api/coding/paas/v4",
        "api.z.ai",
        "ZAI_CODING_API_KEY",
    )
};

pub const KIMI: Row = Row {
    catalog: &[
        "kimi-k3",
        "kimi-k2.7-code",
        "kimi-k2.7-code-highspeed",
        "kimi-k2.6",
        "kimi-k2.5",
        "moonshot-v1-128k",
        "moonshot-v1-32k",
        "moonshot-v1-8k",
        "moonshot-v1-128k-vision-preview",
        "moonshot-v1-32k-vision-preview",
        "moonshot-v1-8k-vision-preview",
    ],
    context_windows: &[
        ("kimi-k3", 1_000_000),
        ("kimi-k2.7", 262_144),
        ("kimi-k2.6", 262_144),
        ("kimi-k2.5", 262_144),
        ("moonshot-v1-128k-vision-preview", 128_000),
        ("moonshot-v1-32k-vision-preview", 32_000),
        ("moonshot-v1-8k-vision-preview", 8_000),
        ("moonshot-v1-128k", 128_000),
        ("moonshot-v1-32k", 32_000),
        ("moonshot-v1-8k", 8_000),
    ],
    vision_filter: Some(kimi_vision_model),
    efforts: Some(kimi_efforts),
    catalog_only: true,
    metadata: ProviderMetadata {
        env_var: Some("MOONSHOT_API_KEY"),
        validation: "catalog-only",
        endpoint: None,
        oauth: Some("Kimi Code OAuth is provider id kimi-code"),
        login_endpoint: None,
        setup: &[
            "Kimi Platform API key provider.",
            "For Kimi Code OAuth, use `goat provider login kimi-code`.",
            "API-key setup: `goat provider login kimi --key sk-...`.",
        ],
    },
    ..Row::hosted(
        "kimi",
        "https://api.moonshot.ai/v1",
        "api.moonshot.ai",
        "MOONSHOT_API_KEY",
    )
};

const QWEN_DEFAULT_ENDPOINT: &str = "https://dashscope-us.aliyuncs.com/compatible-mode/v1";

pub const QWEN: Row = Row {
    catalog: &[
        "qwen3.7-max",
        "qwen3.7-plus",
        "qwen3.6-flash",
        "qwen3-coder-plus",
        "qwen3-coder-flash",
    ],
    context_windows: &[
        ("qwen3.7", 1_000_000),
        ("qwen3.6-flash", 1_000_000),
        ("qwen3-coder", 1_000_000),
    ],
    vision_filter: Some(qwen_vision_model),
    efforts: Some(no_efforts),
    reasoning_effort: false,
    metadata: ProviderMetadata {
        env_var: Some("DASHSCOPE_API_KEY"),
        validation: "network",
        endpoint: Some("required for non-US DashScope workspaces"),
        oauth: Some("Qwen OAuth enrollment discontinued"),
        login_endpoint: Some(LoginEndpointMetadata {
            env_var: Some("QWEN_BASE_URL"),
            default: Some(QWEN_DEFAULT_ENDPOINT),
            validate: Some(validate_qwen_endpoint),
        }),
        setup: &[
            "Qwen DashScope API-key provider.",
            "Default endpoint: https://dashscope-us.aliyuncs.com/compatible-mode/v1",
            "Non-US workspaces: `goat provider login qwen --endpoint <url> --key sk-...`.",
            "Qwen OAuth enrollment is discontinued upstream.",
        ],
    },
    ..Row::hosted(
        "qwen",
        QWEN_DEFAULT_ENDPOINT,
        "dashscope-us.aliyuncs.com",
        "DASHSCOPE_API_KEY",
    )
};

pub const MINIMAX: Row = Row {
    catalog: &[
        "MiniMax-M3",
        "MiniMax-M2.7",
        "MiniMax-M2.7-highspeed",
        "MiniMax-M2.5",
        "MiniMax-M2.5-highspeed",
        "MiniMax-M2.1",
        "MiniMax-M2.1-highspeed",
        "MiniMax-M2",
    ],
    context_windows: &[("MiniMax-M3", 1_000_000), ("MiniMax-M2", 204_800)],
    vision_filter: Some(no_vision),
    efforts: Some(no_efforts),
    images: false,
    reasoning_effort: false,
    catalog_only: true,
    metadata: ProviderMetadata {
        env_var: Some("MINIMAX_API_KEY"),
        validation: "catalog-only",
        endpoint: Some("https://api.minimax.io/v1"),
        oauth: Some("not supported"),
        login_endpoint: None,
        setup: &[
            "MiniMax open platform API-key provider.",
            "Use `MINIMAX_API_KEY` or `goat provider login minimax --key ...`.",
            "Keys from platform.minimax.io are region-scoped; the China platform uses a different host.",
        ],
    },
    ..Row::hosted(
        "minimax",
        "https://api.minimax.io/v1",
        "api.minimax.io",
        "MINIMAX_API_KEY",
    )
};

pub const VERCEL: Row = Row {
    catalog: &[
        "anthropic/claude-opus-5",
        "anthropic/claude-sonnet-5",
        "openai/gpt-5.6",
        "google/gemini-3.6-flash",
        "zai/glm-5.2",
        "moonshotai/kimi-k3",
        "xai/grok-4.5",
    ],
    context_windows: &[
        ("anthropic/claude-opus-5", 1_000_000),
        ("anthropic/claude-sonnet-5", 1_000_000),
        ("openai/gpt-5.6", 1_050_000),
        ("google/gemini-3.6-flash", 1_048_576),
        ("zai/glm-5.2", 1_000_000),
        ("moonshotai/kimi-k3", 1_000_000),
        ("xai/grok-4.5", 500_000),
    ],
    model_filter: Some(vercel_chat_model),
    vision_filter: Some(gateway_vision_model),
    reasoning_effort: false,
    live_model_list: true,
    metadata: ProviderMetadata {
        env_var: Some("AI_GATEWAY_API_KEY"),
        validation: "network",
        endpoint: Some("https://ai-gateway.vercel.sh/v1"),
        oauth: Some("not supported"),
        login_endpoint: None,
        setup: &[
            "Vercel AI Gateway: one key fronting every upstream provider.",
            "Use `AI_GATEWAY_API_KEY` or `goat provider login vercel --key vck_...`.",
            "Models are addressed as `creator/model`, for example `anthropic/claude-opus-5`.",
        ],
    },
    ..Row::hosted(
        "vercel",
        "https://ai-gateway.vercel.sh/v1",
        "ai-gateway.vercel.sh",
        "AI_GATEWAY_API_KEY",
    )
};

pub const OLLAMA: Row = Row::local("ollama", "http://localhost:11434/v1");
pub const LMSTUDIO: Row = Row::local("lmstudio", "http://localhost:1234/v1");
pub const LLAMA_CPP: Row = Row::local("llama-cpp", "http://localhost:8080/v1");

const OPENAI_NON_CHAT_MARKERS: [&str; 15] = [
    "image",
    "audio",
    "tts",
    "whisper",
    "transcribe",
    "realtime",
    "embedding",
    "moderation",
    "search",
    "dall-e",
    "instruct",
    "babbage",
    "davinci",
    "sora",
    "computer-use",
];

fn openai_chat_model(id: &str) -> bool {
    let id = id.to_ascii_lowercase();
    if OPENAI_NON_CHAT_MARKERS
        .iter()
        .any(|marker| id.contains(marker))
    {
        return false;
    }
    let mut chars = id.chars();
    id.starts_with("gpt-")
        || (chars.next() == Some('o') && chars.next().is_some_and(|c| c.is_ascii_digit()))
}

fn openrouter_chat_model(id: &str) -> bool {
    let id = id.to_ascii_lowercase();
    !(id.contains("embedding")
        || id.contains("moderation")
        || id.contains("image")
        || id.contains("tts")
        || id.contains("whisper"))
}

fn vercel_chat_model(id: &str) -> bool {
    let id = id.to_ascii_lowercase();
    !(id.contains("embedding")
        || id.contains("moderation")
        || id.contains("image")
        || id.contains("tts")
        || id.contains("whisper")
        || id.contains("transcribe"))
}

fn gateway_vision_model(id: &str) -> bool {
    let id = id.to_ascii_lowercase();
    known_openai_compatible_vision_model(&id)
        || id.contains("claude")
        || id.contains("gemini")
        || id.contains("grok-4")
}

fn groq_chat_model(id: &str) -> bool {
    let id = id.to_ascii_lowercase();
    !(id.contains("whisper") || id.contains("tts") || id.contains("embedding"))
}

fn mistral_vision_model(id: &str) -> bool {
    let id = id.to_ascii_lowercase();
    id.contains("pixtral")
        || id.starts_with("mistral-large")
        || id.starts_with("mistral-medium")
        || id.starts_with("mistral-small")
}

fn zai_vision_model(id: &str) -> bool {
    let id = id.to_ascii_lowercase();
    id.contains("glm-5v")
        || id.contains("glm-4.6v")
        || id.contains("glm-4.5v")
        || id.contains("glm-4v")
        || id.contains("vision")
}

fn zai_efforts(model: &str) -> Vec<Effort> {
    if model == "glm-5.2" {
        vec![
            Effort::Off,
            Effort::Low,
            Effort::Medium,
            Effort::High,
            Effort::Xhigh,
            Effort::Max,
        ]
    } else {
        Vec::new()
    }
}

fn zai_effort_wire(effort: Effort) -> Option<&'static str> {
    let wire = match effort {
        Effort::Off => "none",
        Effort::Low => "low",
        Effort::Medium => "medium",
        Effort::High => "high",
        Effort::Xhigh => "xhigh",
        Effort::Max => "max",
    };
    (!wire.is_empty()).then_some(wire)
}

fn kimi_vision_model(id: &str) -> bool {
    let id = id.to_ascii_lowercase();
    id.starts_with("kimi-k3") || id.starts_with("kimi-k2.6") || id.contains("vision-preview")
}

fn kimi_efforts(model: &str) -> Vec<Effort> {
    if model.starts_with("kimi-k3") {
        vec![Effort::Low, Effort::High, Effort::Max]
    } else {
        Vec::new()
    }
}

fn qwen_vision_model(id: &str) -> bool {
    let id = id.to_ascii_lowercase();
    id.contains("qwen3.7")
        || id.contains("qwen-vl")
        || id.contains("qwen2-vl")
        || id.contains("qwen2.5-vl")
}

pub fn validate_qwen_endpoint(endpoint: &str) -> Result<String, String> {
    let trimmed = endpoint.trim().trim_end_matches('/');
    let url = reqwest::Url::parse(trimmed).map_err(|err| err.to_string())?;
    if url.scheme() != "https" {
        return Err("qwen endpoint must use https".to_owned());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("qwen endpoint must not include userinfo".to_owned());
    }
    let Some(host) = url.host_str() else {
        return Err("qwen endpoint must include a host".to_owned());
    };
    if host.ends_with('.') {
        return Err("qwen endpoint host must not end with a dot".to_owned());
    }
    let allowed_static = [
        "dashscope.aliyuncs.com",
        "dashscope-intl.aliyuncs.com",
        "dashscope-us.aliyuncs.com",
    ];
    let allowed_regions = [
        "cn-beijing.maas.aliyuncs.com",
        "ap-southeast-1.maas.aliyuncs.com",
        "ap-northeast-1.maas.aliyuncs.com",
    ];
    let allowed = allowed_static.contains(&host)
        || allowed_regions.iter().any(|region| {
            host.strip_suffix(region)
                .and_then(|prefix| prefix.strip_suffix('.'))
                .is_some_and(valid_workspace_id)
        });
    if !allowed {
        return Err("qwen endpoint host is not an allowed Alibaba Model Studio host".to_owned());
    }
    if url.port().is_some() {
        return Err("qwen endpoint must not include a custom port".to_owned());
    }
    if url.path() != "/compatible-mode/v1" {
        return Err("qwen endpoint path must be /compatible-mode/v1".to_owned());
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err("qwen endpoint must not include query or fragment".to_owned());
    }
    Ok(trimmed.to_owned())
}

fn valid_workspace_id(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

#[cfg(test)]
mod tests {
    use goat_auth::{Credential, CredentialKey, CredentialStore, SecretString};

    use super::*;
    use crate::build;

    fn store(name: &str) -> CredentialStore {
        let _ = std::fs::remove_file(std::env::temp_dir().join(name));
        CredentialStore::new(std::env::temp_dir().join(name))
    }

    #[test]
    fn openai_filter_keeps_chat_models() {
        for id in [
            "gpt-5.5",
            "gpt-5-codex",
            "gpt-4o",
            "gpt-4.1-mini",
            "o3",
            "o4-mini",
            "gpt-3.5-turbo",
        ] {
            assert!(openai_chat_model(id), "expected to keep {id}");
        }
    }

    #[test]
    fn openai_filter_drops_non_chat_models() {
        for id in [
            "gpt-4o-mini-tts",
            "gpt-4o-transcribe",
            "whisper-1",
            "tts-1-hd",
            "gpt-image-1",
            "text-embedding-3-large",
            "gpt-realtime",
            "gpt-4o-search-preview",
            "gpt-3.5-turbo-instruct",
            "omni-moderation-latest",
            "davinci-002",
            "sora-2",
        ] {
            assert!(!openai_chat_model(id), "expected to drop {id}");
        }
    }

    #[test]
    fn gateway_filter_drops_non_chat_models() {
        for id in [
            "openai/text-embedding-3-large",
            "black-forest-labs/flux-image",
            "openai/whisper-1",
        ] {
            assert!(!vercel_chat_model(id), "expected to drop {id}");
        }
        assert!(vercel_chat_model("anthropic/claude-opus-5"));
    }

    #[test]
    fn validates_qwen_endpoints() {
        for endpoint in [
            "https://dashscope-us.aliyuncs.com/compatible-mode/v1",
            "https://dashscope.aliyuncs.com/compatible-mode/v1",
            "https://workspace-1.cn-beijing.maas.aliyuncs.com/compatible-mode/v1",
            "https://abc123.ap-southeast-1.maas.aliyuncs.com/compatible-mode/v1/",
        ] {
            assert_eq!(
                validate_qwen_endpoint(endpoint).unwrap(),
                endpoint.trim_end_matches('/')
            );
        }
        for endpoint in [
            "http://dashscope-us.aliyuncs.com/compatible-mode/v1",
            "https://dashscope-us.aliyuncs.com.evil.test/compatible-mode/v1",
            "https://user@dashscope-us.aliyuncs.com/compatible-mode/v1",
            "https://dashscope-us.aliyuncs.com:444/compatible-mode/v1",
            "https://dashscope-us.aliyuncs.com/v1",
            "https://dashscope-us.aliyuncs.com/compatible-mode/v1?x=1",
            "https://workspace_1.cn-beijing.maas.aliyuncs.com/compatible-mode/v1",
            "https://workspace-1.cn-hangzhou.maas.aliyuncs.com/compatible-mode/v1",
        ] {
            assert!(
                validate_qwen_endpoint(endpoint).is_err(),
                "expected rejection for {endpoint}"
            );
        }
    }

    #[test]
    fn invalid_qwen_endpoint_does_not_authenticate() {
        let store = store("goat-provider-builtin-qwen-invalid.json");
        store
            .store(
                &CredentialKey::model("qwen", "default"),
                Credential::ApiKeyWithEndpoint {
                    secret: SecretString::from("key".to_owned()),
                    endpoint: "https://example.com/compatible-mode/v1".to_owned(),
                },
            )
            .unwrap();
        let provider = build(&QWEN, &store, "default");
        assert!(!provider.authenticated());
    }

    #[test]
    fn qwen_endpoint_credential_authenticates() {
        let store = store("goat-provider-builtin-qwen-valid.json");
        store
            .store(
                &CredentialKey::model("qwen", "default"),
                Credential::ApiKeyWithEndpoint {
                    secret: SecretString::from("key".to_owned()),
                    endpoint: "https://dashscope-us.aliyuncs.com/compatible-mode/v1".to_owned(),
                },
            )
            .unwrap();
        let provider = build(&QWEN, &store, "default");
        assert!(provider.authenticated());
    }

    #[test]
    fn local_rows_have_expected_ids() {
        let store = store("goat-provider-builtin-local.json");
        for (row, id) in [
            (&OLLAMA, "ollama"),
            (&LMSTUDIO, "lmstudio"),
            (&LLAMA_CPP, "llama-cpp"),
        ] {
            let provider = build(row, &store, "default");
            assert_eq!(provider.id().to_string(), id);
            assert!(provider.authenticated());
        }
    }

    #[test]
    fn zai_efforts_gated_to_flagship() {
        assert_eq!(zai_efforts("glm-5.2").len(), 6);
        assert!(zai_efforts("glm-5.1").is_empty());
        assert_eq!(zai_effort_wire(Effort::Off), Some("none"));
        assert_eq!(zai_effort_wire(Effort::Xhigh), Some("xhigh"));
    }
}
