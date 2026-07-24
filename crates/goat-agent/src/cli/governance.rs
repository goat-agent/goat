use anyhow::{Context, Result};
use goat_config::GoatPaths;
use goat_store::{SqliteStore, Store};

use super::ui::{self, Footer, Palette, Table};

pub fn status() -> Result<()> {
    let paths = GoatPaths::default_layout()?;
    let daemon_up = goat_config::socket_path().is_some_and(|p| p.exists());
    let agents = goat_config::load_from(paths.clone())
        .map(|c| c.agents)
        .unwrap_or_default();

    ui::cell("Status", || {
        ui::pair("daemon", if daemon_up { "running" } else { "stopped" });
        ui::blank();
        ui::section("Agents");
        if agents.is_empty() {
            ui::line(&ui::dim("none — run `goat agent add`"));
        } else {
            let mut table = Table::new(["agent", "model", "channels"]);
            for a in &agents {
                let channels = if a.bindings.is_empty() {
                    "—".to_owned()
                } else {
                    a.bindings
                        .iter()
                        .map(|b| b.name.clone())
                        .collect::<Vec<_>>()
                        .join(", ")
                };
                table.styled_row(vec![
                    (a.slug.clone(), Palette::Plain),
                    (a.default_model.to_string(), Palette::Muted),
                    (channels, Palette::Muted),
                ]);
            }
            table.render();
        }
        Ok(Footer::None)
    })
}

pub async fn log(limit: usize) -> Result<()> {
    let store = open().await?;
    let rows = store
        .recent_tool_invocations(limit)
        .await
        .context("recent tool invocations")?;
    ui::cell("Log", || {
        if rows.is_empty() {
            ui::line(&ui::dim("no actions recorded yet"));
            return Ok(Footer::None);
        }
        let mut table = Table::new(["when", "tool", "status", "detail"]);
        for r in &rows {
            let style = if r.status == "error" {
                Palette::Warning
            } else {
                Palette::Success
            };
            table.styled_row(vec![
                (
                    r.started_at.format("%m-%d %H:%M").to_string(),
                    Palette::Muted,
                ),
                (r.tool_name.clone(), Palette::Plain),
                (r.status.clone(), style),
                (
                    truncate(r.detail.as_deref().unwrap_or(""), 60),
                    Palette::Muted,
                ),
            ]);
        }
        table.render();
        Ok(Footer::None)
    })
}

async fn open() -> Result<SqliteStore> {
    let paths = GoatPaths::default_layout()?;
    SqliteStore::open(&paths.state_db)
        .await
        .context("open store")
}

fn truncate(s: &str, n: usize) -> String {
    let mut out: String = s.chars().take(n).collect();
    if s.chars().count() > n {
        out.push('…');
    }
    out.replace('\n', " ")
}
