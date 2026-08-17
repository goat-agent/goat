use super::ui;

pub async fn config_changed(agent: Option<&str>) {
    let Some(socket_path) = goat_config::socket_path() else {
        return;
    };
    let ours = goat_client::mine();
    match goat_client::greet(&socket_path).await {
        goat_client::Daemon::Absent => return,
        goat_client::Daemon::Silent => {
            ui::warning("the running daemon is not answering; run `goat daemon stop` and retry");
            return;
        }
        goat_client::Daemon::Reachable(them) => {
            if !goat_client::is_current(&ours, &them) {
                ui::line(&ui::dim(
                    "saved — the running daemon is a different build; run `goat daemon start` to apply",
                ));
                return;
            }
        }
    }
    let Ok(daemon_exe) = std::env::current_exe() else {
        return;
    };
    let link = goat_client::Link::local(socket_path, daemon_exe);
    match goat_client::reload(&link, agent.map(str::to_owned)).await {
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
