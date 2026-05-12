use anyhow::Result;
use goat_config::GoatPaths;
use goat_llm::{LlmProviderFactory, ProviderId};
use serde_json::{json, Value};

use super::edit_credentials;
use super::ui::{self, Footer};

pub async fn run() -> Result<()> {
    ui::cell("Setup", || {
        let paths = GoatPaths::default_layout()?;
        std::fs::create_dir_all(&paths.root)?;
        std::fs::create_dir_all(&paths.personas_dir)?;
        std::fs::create_dir_all(&paths.skills_dir)?;
        std::fs::create_dir_all(&paths.logs_dir)?;
        ui::pair("root", &paths.root.display().to_string());
        ui::blank();

        provider_loop(&paths)?;

        ui::blank();
        let slug = super::persona::create_interactive(&paths)?;

        let next = format!("goat persona channel add {slug}");
        Ok(Footer::Hint("Done", next))
    })?;
    Ok(())
}

fn provider_loop(paths: &GoatPaths) -> Result<()> {
    ui::section("Providers");
    loop {
        let mut items: Vec<(Option<ProviderId>, String)> = inventory::iter::<LlmProviderFactory>()
            .map(|f| (Some(f.id.clone()), f.id.as_str().to_string()))
            .collect();
        items.sort_by(|a, b| a.1.cmp(&b.1));
        items.push((None, "done".into()));

        let pick = ui::pick("add provider", &items)?;
        let Some(provider) = pick else { break };

        let api_key = ui::secret(&format!("{} key", provider.as_str()))?;
        edit_credentials(&paths.credentials_json, |map| {
            let entry = json!({ "api_key": api_key });
            let list = map
                .entry(provider.as_str().to_string())
                .or_insert_with(|| Value::Array(Vec::new()));
            if let Value::Array(arr) = list {
                arr.push(entry);
            }
        })?;
        ui::pair(provider.as_str(), "saved");
    }
    Ok(())
}
