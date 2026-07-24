use goat_provider_openai_compat::OpenAiCompatProvider;

pub fn ollama() -> OpenAiCompatProvider {
    OpenAiCompatProvider::local("ollama", "http://localhost:11434/v1")
}

pub fn lmstudio() -> OpenAiCompatProvider {
    OpenAiCompatProvider::local("lmstudio", "http://localhost:1234/v1")
}

pub fn llama_cpp() -> OpenAiCompatProvider {
    OpenAiCompatProvider::local("llama-cpp", "http://localhost:8080/v1")
}

#[cfg(test)]
mod tests {
    use goat_provider::{Provider, ProviderId};

    use super::{llama_cpp, lmstudio, ollama};

    #[test]
    fn ollama_has_correct_id() {
        assert_eq!(ollama().id(), ProviderId::from("ollama"));
    }

    #[test]
    fn lmstudio_has_correct_id() {
        assert_eq!(lmstudio().id(), ProviderId::from("lmstudio"));
    }

    #[test]
    fn llama_cpp_has_correct_id() {
        assert_eq!(llama_cpp().id(), ProviderId::from("llama-cpp"));
    }
}
