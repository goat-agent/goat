pub(crate) fn probe(key: String) -> goat_llm::ProbeFuture {
    Box::pin(async move {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(8))
            .build()
            .map_err(|e| short_err(&e.to_string()))?;
        let resp = client
            .get("https://api.anthropic.com/v1/models")
            .header("x-api-key", &key)
            .header("anthropic-version", "2023-06-01")
            .send()
            .await
            .map_err(|e| short_err(&e.to_string()))?;
        if resp.status().is_success() {
            Ok(())
        } else {
            Err(format!("http {}", resp.status().as_u16()))
        }
    })
}

fn short_err(s: &str) -> String {
    s.split(':').next().unwrap_or(s).trim().to_string()
}
