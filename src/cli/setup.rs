use std::sync::Arc;

use anyhow::Result;
use goat_config::GoatPaths;
use goat_credentials::JsonFileStore;
use goat_llm::{CredentialStore, LlmProviderSpec, SetupCtx, UserPrompt};

use super::ui;
use super::CliPrompt;

pub async fn run() -> Result<()> {
    let paths = GoatPaths::default_layout()?;
    std::fs::create_dir_all(&paths.root)?;
    std::fs::create_dir_all(&paths.personas_dir)?;
    std::fs::create_dir_all(&paths.skills_dir)?;
    std::fs::create_dir_all(&paths.logs_dir)?;

    let store: Arc<dyn CredentialStore> =
        Arc::new(JsonFileStore::open(paths.credentials_json.clone())?);
    let prompt: Arc<dyn UserPrompt> = Arc::new(CliPrompt);

    ui::pair("root", &paths.root.display().to_string());
    ui::blank();
    provider_loop(&store, prompt).await?;
    ui::blank();
    let slug = super::persona::create_interactive(&paths)?;
    ui::pair("done", &format!("goat persona channel add {slug}"));
    Ok(())
}

async fn provider_loop(
    store: &Arc<dyn CredentialStore>,
    prompt: Arc<dyn UserPrompt>,
) -> Result<()> {
    ui::section("Providers");
    loop {
        let mut items: Vec<(Option<&'static LlmProviderSpec>, String)> =
            inventory::iter::<LlmProviderSpec>()
                .map(|s| (Some(s), s.id.as_str().to_string()))
                .collect();
        items.sort_by(|a, b| a.1.cmp(&b.1));
        items.push((None, "done".into()));

        let pick = ui::pick("add provider", &items)?;
        let Some(spec) = pick else { break };

        let ctx = SetupCtx {
            provider: spec.id.clone(),
            label: None,
            prompt: prompt.clone(),
        };
        match spec.setup.run(ctx).await {
            Ok(value) => {
                store.write(spec.id.clone(), None, value.clone())?;
                ui::pair(spec.id.as_str(), &(spec.summarize)(&value));
            }
            Err(e) => {
                ui::line(&ui::dim(&format!("skipped {}: {e}", spec.id.as_str())));
            }
        }
    }
    Ok(())
}
