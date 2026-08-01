use super::ui;

pub async fn config_changed(agent: Option<&str>) {
    let Some(socket_path) = goat_config::socket_path() else {
        return;
    };
    if !goat_wire::transport::probe_alive(&socket_path) {
        return;
    }
    match goat_client::reload(&socket_path, agent.map(str::to_owned)).await {
        Ok(report) => {
            for failure in &report.failed {
                ui::warning(&format!("{}: {}", failure.agent, failure.reason));
            }
            for warning in &report.warnings {
                ui::warning(warning);
            }
            if !report.reloaded.is_empty() {
                ui::line(&ui::dim(&format!(
                    "applied to the running daemon: {}",
                    report.reloaded.join(", ")
                )));
            }
        }
        Err(e) => ui::warning(&format!(
            "could not apply to the running daemon: {e}; run `goat reload`"
        )),
    }
}
