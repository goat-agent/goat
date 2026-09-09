mod backend;
mod computer;
mod link;
mod sessions;

use tauri::Manager;
use tracing_subscriber::{EnvFilter, layer::SubscriberExt, util::SubscriberInitExt};

fn logging() -> Result<tracing_appender::non_blocking::WorkerGuard, Box<dyn std::error::Error>> {
    let directory = goat_config::log_dir().ok_or(goat_config::HOME_NOT_FOUND)?;
    std::fs::create_dir_all(&directory)?;
    let (writer, guard) =
        tracing_appender::non_blocking(tracing_appender::rolling::daily(directory, "desktop.log"));
    let filter = std::env::var("GOAT_LOG")
        .ok()
        .and_then(|filter| EnvFilter::try_new(filter).ok())
        .unwrap_or_else(|| EnvFilter::new("info"));
    tracing_subscriber::registry()
        .with(filter)
        .with(
            tracing_subscriber::fmt::layer()
                .with_writer(writer)
                .with_ansi(false),
        )
        .try_init()?;
    Ok(guard)
}

fn main() -> std::process::ExitCode {
    let Ok(_logging) = logging() else {
        return std::process::ExitCode::FAILURE;
    };
    match run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            tracing::error!(%error, "desktop startup failed");
            std::process::ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let link = link::local().map_err(std::io::Error::other)?;
    let computer = computer::ComputerState::new(link.clone()).map_err(std::io::Error::other)?;
    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .manage(sessions::Sessions::new(link.clone()))
        .manage(backend::Backend::new(link))
        .manage(computer);
    #[cfg(target_os = "macos")]
    let builder = builder.plugin(tauri_plugin_macos_permissions::init());
    builder
        .invoke_handler(tauri::generate_handler![
            backend::daemon_status,
            backend::projects_list,
            backend::projects_add,
            backend::projects_remove,
            backend::conversations,
            backend::workspace,
            backend::agents,
            backend::agent_send,
            backend::agent_chat_open,
            backend::agent_chat_close,
            backend::agent_activity_open,
            backend::agent_activity_close,
            backend::agent_schedules,
            sessions::session_open,
            sessions::session_listen,
            sessions::session_close,
            sessions::session_op,
            sessions::session_submit,
            sessions::session_command,
            sessions::session_admin,
            sessions::command_specs,
            sessions::session_state,
            computer::computer_status,
            computer::computer_request_permission,
            computer::computer_resume,
        ])
        .setup(|app| {
            computer::setup(app.handle()).map_err(std::io::Error::other)?;
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                if let Err(error) = handle.state::<backend::Backend>().connection().await {
                    tracing::warn!(%error, "desktop could not connect to daemon");
                }
            });
            Ok(())
        })
        .build(tauri::generate_context!())?
        .run(|app, event| {
            if matches!(event, tauri::RunEvent::Exit) {
                computer::stop(app);
            }
        });
    Ok(())
}
