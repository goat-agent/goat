use std::path::PathBuf;
use std::sync::Arc;

use anyhow::anyhow;
use futures::StreamExt;
use goat_credentials::JsonFileStore;
use goat_llm::{CredentialStore, LlmChunk, LlmMessage, LlmProviderSpec, LlmRequest, Model};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let home = std::env::var("HOME")?;
    let path: PathBuf = format!("{home}/.goat/credentials.json").into();
    let store: Arc<dyn CredentialStore> = Arc::new(JsonFileStore::open(path)?);

    let spec = inventory::iter::<LlmProviderSpec>()
        .find(|s| s.id == goat_llm_codex::ID)
        .ok_or_else(|| anyhow!("codex spec not registered (forgot extern?)"))?;
    let provider = (spec.build)(store);

    let model_id = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "gpt-5-codex".into());
    eprintln!("model: {model_id}");
    let model = Model::new(goat_llm_codex::ID, model_id);
    let mut req = LlmRequest::new(model);
    req.system = Some("Respond very briefly.".into());
    req.messages
        .push(LlmMessage::user_text("Say hi in five words."));

    eprintln!("→ requesting codex stream...");
    let mut stream = provider.stream(req).await?;
    while let Some(item) = stream.next().await {
        match item {
            Ok(LlmChunk::TextDelta { text, .. }) => {
                use std::io::Write;
                print!("{text}");
                std::io::stdout().flush().ok();
            }
            Ok(LlmChunk::ReasoningDelta { text, .. }) => {
                eprintln!("[reasoning] {text}");
            }
            Ok(LlmChunk::MessageStart { id, model, .. }) => {
                eprintln!("← start id={id} model={model}");
            }
            Ok(LlmChunk::MessageEnd { stop, usage }) => {
                eprintln!("\n← end stop={stop:?} usage={usage:?}");
            }
            Ok(other) => eprintln!("← {other:?}"),
            Err(e) => {
                eprintln!("\n!! error: {e}");
                break;
            }
        }
    }
    Ok(())
}
