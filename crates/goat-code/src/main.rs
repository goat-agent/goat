mod auth;
mod cli;
mod headless;
mod logging;
mod proxy_ops;
mod search;
mod ui;
mod update;

use clap::{CommandFactory, FromArgMatches};
use color_eyre::eyre::eyre;

use crate::ui::{ColorMode, Palette, pair};

use crate::cli::{
    Cli, CodeArgs, CodeCommand, Command, DaemonCommand, RemoteCommand, WorktreeCommand,
};

use goat_agent_command_skill as _;
use goat_agent_tool_fs as _;
use goat_agent_tool_shell as _;
use goat_agent_tool_skill as _;
use goat_channel_discord as _;
use goat_integration_linear as _;
use goat_integration_slack as _;

fn into_eyre(err: &anyhow::Error) -> color_eyre::Report {
    eyre!(err.to_string())
}

#[tokio::main]
async fn main() -> color_eyre::Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let matches = Cli::command().get_matches();
    let cli = Cli::from_arg_matches(&matches).map_err(|e| eyre!(e.to_string()))?;

    match cli.command {
        None => {
            Cli::command().print_help()?;
            Ok(())
        }
        Some(Command::Code(args)) => run_code(args).await,
        Some(Command::Agent(c)) => goat_agent::cli::agent::run(c)
            .await
            .map_err(|e| into_eyre(&e)),
        Some(Command::Integration(c)) => goat_agent::cli::integration::run_connect(c)
            .await
            .map_err(|e| into_eyre(&e)),
        Some(Command::Setup) => auth::run_setup().await,
        Some(Command::Doctor(args)) => goat_agent::cli::doctor::run(args)
            .await
            .map_err(|e| into_eyre(&e)),
        Some(Command::Update { force }) => update::run(force).await,
        Some(Command::Provider(command)) => auth::run_provider(command).await,
        Some(Command::Daemon(command)) => run_daemon_command(command).await,
        Some(Command::Remote(command)) => run_remote_command(command).await,
    }
}

async fn run_code(args: CodeArgs) -> color_eyre::Result<()> {
    match args.command {
        Some(CodeCommand::Worktree(command)) => {
            let result = match command {
                WorktreeCommand::List => goat_worktree::list(),
                WorktreeCommand::Remove { label } => goat_worktree::remove(&label),
            };
            return result.map_err(color_eyre::Report::from);
        }
        Some(CodeCommand::Search(command)) => return search::run(command),
        None => {}
    }
    if args.print_log_path {
        if let Some(dir) = goat_config::log_dir() {
            println!("{}", dir.display());
        }
        return Ok(());
    }
    if args.headless || args.print {
        run_headless(args.worktree, args.r#continue, &args.protocol, args.print).await
    } else {
        run_tui(args.worktree, args.r#continue).await
    }
}

async fn connect_session(
    worktree_label: Option<String>,
    r#continue: bool,
) -> color_eyre::Result<goat_client::Attachment> {
    let cwd = if let Some(label) = worktree_label.as_deref() {
        goat_worktree::enter(label)?
    } else {
        std::env::current_dir()?
    };

    let socket_path = goat_config::socket_path()
        .ok_or_else(|| color_eyre::eyre::eyre!(goat_config::HOME_NOT_FOUND))?;
    let daemon_exe = std::env::current_exe()?;
    let resume = if r#continue {
        goat_wire::ResumeMode::Latest {}
    } else {
        goat_wire::ResumeMode::New {}
    };

    goat_client::connect(&socket_path, &daemon_exe, cwd, resume)
        .await
        .map_err(color_eyre::Report::from)
}

async fn run_tui(worktree_label: Option<String>, r#continue: bool) -> color_eyre::Result<()> {
    goat_tui::install_hooks()?;
    let _guard = logging::init();

    let config = goat_config::Config::load();
    let theme = match config.theme {
        goat_config::ThemeChoice::Dark => goat_tui::Theme::dark(),
        goat_config::ThemeChoice::Light => goat_tui::Theme::light(),
    };

    let attachment = connect_session(worktree_label, r#continue).await?;
    let goat_client::Attachment {
        ops,
        events,
        presence,
        pump,
        ..
    } = attachment;

    goat_tui::run(ops, events, presence, theme, Vec::new()).await?;
    pump.abort();
    Ok(())
}

async fn run_headless(
    worktree_label: Option<String>,
    r#continue: bool,
    protocol: &str,
    one_shot: bool,
) -> color_eyre::Result<()> {
    let _guard = logging::init();

    let codec = headless::codec_for(protocol)?;
    let attachment = connect_session(worktree_label, r#continue).await?;
    let goat_client::Attachment {
        ops, events, pump, ..
    } = attachment;

    let exit = headless::run(ops, events, codec, one_shot).await;
    pump.abort();
    match exit {
        headless::Exit::Ok => std::process::exit(0),
        headless::Exit::Disconnected => {
            eprintln!("headless: daemon connection closed");
            std::process::exit(1);
        }
    }
}

fn install_daemon_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let location = info.location().map_or_else(
            || "unknown".to_owned(),
            |l| format!("{}:{}:{}", l.file(), l.line(), l.column()),
        );
        let message = info.payload().downcast_ref::<&str>().map_or_else(
            || {
                info.payload()
                    .downcast_ref::<String>()
                    .map_or("<non-string panic payload>", String::as_str)
                    .to_owned()
            },
            |s| (*s).to_owned(),
        );
        tracing::error!(location, message, "daemon panicked");
        previous(info);
    }));
}

async fn run_unified_daemon(socket_path: std::path::PathBuf) -> color_eyre::Result<()> {
    use tokio_util::sync::CancellationToken;

    install_daemon_panic_hook();
    if goat_daemon::already_running(&socket_path) {
        eprintln!("a daemon is already running at {}", socket_path.display());
        std::process::exit(1);
    }
    let home_err = || color_eyre::eyre::eyre!(goat_config::HOME_NOT_FOUND);
    let paths = goat_config::GoatPaths::default_layout().map_err(|e| color_eyre::eyre::eyre!(e))?;
    let auth_path = paths.credentials_json.clone();
    let db_path = paths.state_db.clone();
    let remote = remote_settings()?;

    let shutdown = CancellationToken::new();
    {
        let shutdown = shutdown.clone();
        tokio::spawn(async move {
            wait_for_signal().await;
            tracing::info!("received termination signal, shutting down");
            shutdown.cancel();
        });
    }

    let manager = goat_daemon::Manager::new(auth_path, db_path.clone());

    let proxy_config = goat_config::Config::load().proxy;
    let mut agent_meter = None;
    let mut proxy_http = None;
    if proxy_config.enabled {
        match goat_store::ProxyStore::open(&db_path).await {
            Ok(proxy_store) => {
                let (recorder, _writer) = goat_proxy::Recorder::spawn(proxy_store.clone());
                manager.set_meter(goat_proxy::Meter::new(
                    goat_proxy::SOURCE_CODE,
                    recorder.clone(),
                ));
                agent_meter = Some(goat_proxy::Meter::new(goat_proxy::SOURCE_AGENT, recorder));
                let creds = goat_auth::CredentialStore::new(paths.credentials_json.clone());
                let ops = proxy_ops::RegistryAccountOps::new(creds.clone());
                proxy_http = Some((proxy_store, proxy_config.bind, creds, ops));
            }
            Err(err) => tracing::warn!(%err, "proxy store unavailable; usage metering disabled"),
        }
    }

    let goat = goat_runtime::Goat::boot_with_code_metered(Some(manager.clone()), agent_meter)
        .await
        .map_err(|e| color_eyre::eyre::eyre!(e.to_string()))?;
    let agent = tokio::spawn(goat.run_until(shutdown.clone()));

    if let Some((proxy_store, bind, creds, ops)) = proxy_http {
        if let Some(rl_path) = goat_config::rate_limits_path() {
            let backfilled = goat_proxy::backfill_rate_limits(&proxy_store, &rl_path).await;
            if backfilled > 0 {
                tracing::info!(backfilled, "proxy rate limits backfilled");
            }
        }
        match bind.parse::<std::net::SocketAddr>() {
            Ok(bind) => {
                let shutdown = shutdown.clone();
                tokio::spawn(async move {
                    if let Err(err) =
                        goat_proxy::serve(bind, proxy_store, creds, ops, shutdown).await
                    {
                        tracing::warn!(%err, "proxy dashboard stopped");
                    }
                });
            }
            Err(err) => {
                tracing::warn!(%bind, %err, "invalid proxy bind address");
            }
        }
    }

    let config = goat_daemon::DaemonConfig {
        socket_path,
        auth_path: goat_config::auth_path().ok_or_else(home_err)?,
        db_path,
        remote,
    };
    let serve_result = goat_daemon::serve_with(config, manager, shutdown.clone()).await;

    shutdown.cancel();
    if let Err(e) = agent.await {
        tracing::warn!(error = ?e, "agent runtime task join failed");
    }
    serve_result.map_err(color_eyre::Report::from)
}

#[cfg(unix)]
async fn wait_for_signal() {
    use tokio::signal::unix::{SignalKind, signal};
    let (Ok(mut term), Ok(mut int)) = (
        signal(SignalKind::terminate()),
        signal(SignalKind::interrupt()),
    ) else {
        std::future::pending::<()>().await;
        return;
    };
    tokio::select! {
        _ = term.recv() => {}
        _ = int.recv() => {}
    }
}

#[cfg(not(unix))]
async fn wait_for_signal() {
    let _ = tokio::signal::ctrl_c().await;
}

async fn run_daemon_command(command: DaemonCommand) -> color_eyre::Result<()> {
    let socket_path = goat_config::socket_path()
        .ok_or_else(|| color_eyre::eyre::eyre!(goat_config::HOME_NOT_FOUND))?;
    match command {
        DaemonCommand::Serve => run_unified_daemon(socket_path).await,
        DaemonCommand::List => {
            let sessions = goat_client::status(&socket_path).await?;
            let color = ColorMode::detect();
            if sessions.is_empty() {
                println!("{}", color.paint("no live sessions", Palette::Muted));
            } else {
                println!(
                    "  {} {} {} {} {}",
                    color.cell("session", Palette::Muted, 10),
                    color.cell("state", Palette::Muted, 14),
                    color.cell("windows", Palette::Muted, 8),
                    color.cell("age", Palette::Muted, 8),
                    color.paint("cwd", Palette::Muted)
                );
                for session in sessions {
                    let (state, palette) = daemon_state(session.state);
                    println!(
                        "{} {} {} {} {} {}",
                        color.paint("●", palette),
                        color.cell(format!("#{}", session.session.0), Palette::Provider, 10),
                        color.cell(state, palette, 14),
                        color.cell(session.windows.to_string(), Palette::Value, 8),
                        color.cell(format!("{}s", session.age_ms / 1000), Palette::Value, 8),
                        color.paint(session.cwd, Palette::Value)
                    );
                }
            }
            Ok(())
        }
        DaemonCommand::Stop => {
            goat_client::stop(&socket_path).await?;
            println!("daemon stopped");
            Ok(())
        }
        DaemonCommand::Kill { session } => {
            goat_client::kill_session(&socket_path, session).await?;
            println!("killed session #{session}");
            Ok(())
        }
    }
}

fn daemon_state(state: goat_wire::SessionLiveState) -> (&'static str, Palette) {
    match state {
        goat_wire::SessionLiveState::Idle {} => ("idle", Palette::Local),
        goat_wire::SessionLiveState::Active {} => ("active", Palette::Success),
        goat_wire::SessionLiveState::WaitingOnAsk {} => ("waiting", Palette::Warning),
    }
}

fn remote_settings() -> color_eyre::Result<Option<goat_daemon::RemoteSettings>> {
    let config = goat_config::Config::load();
    let Some(remote_dir) = goat_config::remote_dir() else {
        return Ok(None);
    };
    let bind = config
        .remote
        .bind
        .parse()
        .map_err(|e| color_eyre::eyre::eyre!("invalid remote bind address: {e}"))?;
    Ok(Some(goat_daemon::RemoteSettings {
        remote_dir,
        bind,
        advertised: config.remote.advertised,
    }))
}

async fn run_remote_command(command: RemoteCommand) -> color_eyre::Result<()> {
    let socket_path = goat_config::socket_path()
        .ok_or_else(|| color_eyre::eyre::eyre!(goat_config::HOME_NOT_FOUND))?;
    match command {
        RemoteCommand::Pair { label } => {
            let label = label.unwrap_or_else(|| "device".to_owned());
            let info = goat_client::pair_device(&socket_path, label).await?;
            let color = ColorMode::detect();
            println!("{}", color.paint("pairing", Palette::Provider));
            pair("code", &info.code);
            pair("fingerprint", &info.server_fingerprint);
            let address = if info.advertised.is_empty() {
                "none configured".to_owned()
            } else {
                info.advertised.join(", ")
            };
            pair("address", &address);
            print_pairing_qr(&info);
            Ok(())
        }
        RemoteCommand::List => {
            let devices = goat_client::list_devices(&socket_path).await?;
            let color = ColorMode::detect();
            if devices.is_empty() {
                println!("{}", color.paint("no paired devices", Palette::Muted));
            } else {
                println!(
                    "  {} {} {}",
                    color.cell("device", Palette::Muted, 20),
                    color.cell("label", Palette::Muted, 18),
                    color.paint("paired", Palette::Muted)
                );
                for device in devices {
                    println!(
                        "{} {} {} {}",
                        color.paint("●", Palette::Success),
                        color.cell(device.id, Palette::Provider, 20),
                        color.cell(device.label, Palette::Value, 18),
                        color.paint(device.paired_at.to_string(), Palette::Value)
                    );
                }
            }
            Ok(())
        }
        RemoteCommand::Revoke { device } => {
            let ok = goat_client::revoke_device(&socket_path, device.clone()).await?;
            if ok {
                println!("revoked device {device}");
            } else {
                println!("no such device: {device}");
            }
            Ok(())
        }
    }
}

fn print_pairing_qr(info: &goat_client::PairingInfo) {
    let address = info.advertised.first().cloned().unwrap_or_default();
    let payload = format!(
        "goat-pair:code={}&fp={}&addr={}",
        info.code, info.server_fingerprint, address
    );
    match qrcode::QrCode::new(payload.as_bytes()) {
        Ok(code) => {
            let rendered = code
                .render::<qrcode::render::unicode::Dense1x2>()
                .quiet_zone(true)
                .module_dimensions(1, 1)
                .build();
            println!("{rendered}");
        }
        Err(_) => {
            println!("(could not render QR; use the values above)");
        }
    }
}
