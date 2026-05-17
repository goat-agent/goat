use serde_json::Value;

pub(crate) fn probe(v: Value) -> goat_llm::ProbeFuture {
    Box::pin(async move {
        let key = v
            .get("api_key")
            .and_then(|x| x.as_str())
            .ok_or_else(|| "no api_key field".to_string())?
            .to_string();
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(8))
            .build()
            .map_err(|e| short_err(&e.to_string()))?;
        let resp = client
            .get("https://open.bigmodel.cn/api/paas/v4/models")
            .bearer_auth(&key)
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
