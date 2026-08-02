mod auth;
mod cli;
mod headless;
mod logging;
mod mcp;
mod mcp_import;
mod mcp_secrets;
mod proxy_ops;
mod remote;
mod search;
mod ui;
mod update;

use clap::{CommandFactory, FromArgMatches};
use color_eyre::eyre::eyre;

use crate::ui::{ColorMode, Palette, pair};

use crate::cli::{
    Cli, CodeArgs, CodeCommand, Command, DaemonCommand, DeviceCommand, RemoteCommand,
    WorktreeCommand,
};

use goat_agent_command_skill as _;
use goat_agent_tool_fs as _;
use goat_agent_tool_shell as _;
use goat_agent_tool_skill as _;
use goat_channel_discord as _;
use goat_channel_slack as _;
use goat_integration_github as _;
use goat_integration_langfuse as _;
use goat_integration_linear as _;
use goat_integration_notion as _;
use goat_integration_posthog as _;
use goat_integration_sentry as _;
use goat_integration_slack as _;
use goat_integration_tiro as _;

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
        Some(Command::Reload { agent }) => run_reload(agent).await,
        Some(Command::Update { force }) => update::run(force).await,
        Some(Command::Provider(command)) => auth::run_provider(command).await,
        Some(Command::Mcp(command)) => mcp::run(command).await,
        Some(Command::Daemon { remote, command }) => {
            run_daemon_command(command, remote.as_deref()).await
        }
        Some(Command::Device(command)) => run_device_command(command).await,
        Some(Command::Remote(command)) => run_remote_command(command).await,
    }
}

async fn run_reload(agent: Option<String>) -> color_eyre::Result<()> {
    let color = ColorMode::detect();
    let link = remote::local()?;
    let socket_path = goat_config::socket_path()
        .ok_or_else(|| color_eyre::eyre::eyre!(goat_config::HOME_NOT_FOUND))?;
    if !goat_daemon::already_running(&socket_path) {
        println!(
            "{}",
            color.paint(
                "no daemon is running; the new config loads the next time goat starts",
                Palette::Muted,
            )
        );
        return Ok(());
    }

    let report = goat_client::reload(&link, agent)
        .await
        .map_err(|e| color_eyre::eyre::eyre!(e.to_string()))?;

    for warning in &report.warnings {
        println!(
            "{}",
            color.paint(format!("warning: {warning}"), Palette::Warning)
        );
    }
    for failure in &report.failed {
        println!(
            "{}",
            color.paint(
                format!("{}: {}", failure.agent, failure.reason),
                Palette::Warning,
            )
        );
    }
    if !report.reloaded.is_empty() {
        println!(
            "{}",
            color.paint(
                format!("reloaded {}", report.reloaded.join(", ")),
                Palette::Success
            )
        );
    }
    if !report.unchanged.is_empty() {
        println!(
            "{}",
            color.paint(
                format!("unchanged {}", report.unchanged.join(", ")),
                Palette::Muted,
            )
        );
    }
    if report.reloaded.is_empty() && report.unchanged.is_empty() && report.failed.is_empty() {
        println!(
            "{}",
            color.paint("no agents are configured", Palette::Muted)
        );
    }
    if report.failed.is_empty() {
        Ok(())
    } else {
        Err(color_eyre::eyre::eyre!("some agents did not reload"))
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
    let link = std::sync::Arc::new(remote::resolve(args.remote.as_deref())?);
    if args.print_log_path {
        if link.is_local() {
            if let Some(dir) = goat_config::log_dir() {
                println!("{}", dir.display());
            }
        } else {
            ui::warning("the log lives on the daemon host; run this there");
        }
        return Ok(());
    }
    if args.headless || args.print {
        run_headless(&link, &args, args.print).await
    } else {
        run_tui(&link, &args).await
    }
}

async fn connect_session(
    link: &std::sync::Arc<goat_client::Link>,
    args: &CodeArgs,
    approve_project_mcp: bool,
) -> color_eyre::Result<goat_client::Attachment> {
    let cwd = session_cwd(link, args)?;
    if approve_project_mcp && link.is_local() {
        mcp::approve_project_servers(&cwd)?;
    }
    let resume = if args.r#continue {
        goat_wire::ResumeMode::Latest {}
    } else {
        goat_wire::ResumeMode::New {}
    };

    let attachment = goat_client::connect(link.clone(), cwd, resume).await?;
    remote::remember_dir(link, &attachment.cwd);
    Ok(attachment)
}

fn session_cwd(
    link: &goat_client::Link,
    args: &CodeArgs,
) -> color_eyre::Result<std::path::PathBuf> {
    if link.is_local() {
        return match args.worktree.as_deref() {
            Some(label) => goat_worktree::enter(label).map_err(ui::worktree_entry),
            None => std::env::current_dir().map_err(color_eyre::Report::from),
        };
    }
    if args.worktree.is_some() {
        return Err(eyre!(
            "--worktree builds a git worktree on this machine; it does not apply to a remote daemon"
        ));
    }
    if let Some(dir) = args.dir.clone() {
        return Ok(std::path::PathBuf::from(dir));
    }
    remote::last_dir(link.name())
        .map(std::path::PathBuf::from)
        .ok_or_else(|| {
            eyre!(
                "remote `{}` has no directory yet; pass --dir <path on the daemon host>",
                link.name()
            )
        })
}

async fn run_tui(
    link: &std::sync::Arc<goat_client::Link>,
    args: &CodeArgs,
) -> color_eyre::Result<()> {
    goat_tui::install_hooks()?;
    let _guard = logging::init();

    let config = goat_config::Config::load();
    let theme = match config.theme {
        goat_config::ThemeChoice::Dark => goat_tui::Theme::dark(),
        goat_config::ThemeChoice::Light => goat_tui::Theme::light(),
    };

    let attachment = connect_session(link, args, true).await?;
    let managed = link
        .is_local()
        .then(|| {
            std::env::current_dir()
                .ok()
                .and_then(|cwd| goat_worktree::workspace(&cwd).ok())
        })
        .flatten()
        .filter(|workspace| matches!(workspace.kind, goat_worktree::WorkspaceKind::Managed { .. }));
    let goat_client::Attachment {
        ops,
        events,
        presence,
        cwd,
        mut pump,
        ..
    } = attachment;
    let origin = if link.is_local() {
        goat_tui::Origin::local(cwd)
    } else {
        goat_tui::Origin::remote(cwd, link.name().to_owned())
    };

    let exit = goat_tui::run(ops, events, presence, theme, origin, Vec::new()).await?;
    if tokio::time::timeout(std::time::Duration::from_secs(1), &mut pump)
        .await
        .is_err()
    {
        pump.abort();
    }
    if exit == goat_tui::ExitReason::Requested
        && let Some(workspace) = managed
    {
        prompt_worktree_removal(workspace).await?;
    }
    Ok(())
}

async fn prompt_worktree_removal(workspace: goat_worktree::Workspace) -> color_eyre::Result<()> {
    let goat_worktree::WorkspaceKind::Managed { label } = &workspace.kind else {
        return Ok(());
    };
    let choices = ["Keep worktree".to_owned(), "Delete worktree".to_owned()];
    if ui::select_index(&format!("worktree `{label}`"), &choices)? != Some(1) {
        return Ok(());
    }

    let Ok(link) = remote::local() else {
        ui::warning("keeping worktree because the daemon socket is unavailable");
        return Ok(());
    };
    match worktree_has_live_sessions(&link, &workspace.repo_root).await {
        Ok(true) => {
            ui::warning("keeping worktree because another code session is using it");
            return Ok(());
        }
        Err(error) => {
            ui::warning(&format!(
                "keeping worktree because live sessions could not be checked: {error}"
            ));
            return Ok(());
        }
        Ok(false) => {}
    }

    std::env::set_current_dir(&workspace.owner_root)?;
    match goat_worktree::remove(label) {
        Ok(()) => ui::success(&format!("deleted worktree `{label}`")),
        Err(error) => ui::warning(&format!("keeping worktree: {error}")),
    }
    Ok(())
}

async fn worktree_has_live_sessions(
    link: &goat_client::Link,
    root: &std::path::Path,
) -> Result<bool, goat_client::ClientError> {
    for attempt in 0..5 {
        let sessions = goat_client::status(link).await?;
        let in_use = sessions.iter().any(|session| {
            let cwd = std::path::Path::new(&session.cwd);
            cwd.canonicalize()
                .map_or_else(|_| cwd.starts_with(root), |cwd| cwd.starts_with(root))
        });
        if !in_use {
            return Ok(false);
        }
        if attempt < 4 {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    }
    Ok(true)
}

async fn run_headless(
    link: &std::sync::Arc<goat_client::Link>,
    args: &CodeArgs,
    one_shot: bool,
) -> color_eyre::Result<()> {
    let _guard = logging::init();

    let codec = headless::codec_for(&args.protocol)?;
    let attachment = connect_session(link, args, false).await?;
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

    let manager = goat_daemon::Manager::new(
        auth_path,
        goat_config::UserProviders::at(paths.config_json.clone()),
        db_path.clone(),
    );

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
                let ops = proxy_ops::RegistryAccountOps::new(
                    creds.clone(),
                    goat_config::UserProviders::at(paths.config_json.clone()),
                );
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
        config_json: goat_config::config_path().ok_or_else(home_err)?,
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

async fn run_daemon_command(
    command: DaemonCommand,
    target: Option<&str>,
) -> color_eyre::Result<()> {
    match command {
        DaemonCommand::Serve => {
            let socket_path = goat_config::socket_path()
                .ok_or_else(|| color_eyre::eyre::eyre!(goat_config::HOME_NOT_FOUND))?;
            run_unified_daemon(socket_path).await
        }
        DaemonCommand::List => {
            let sessions = goat_client::status(&remote::resolve(target)?).await?;
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
            goat_client::stop(&remote::resolve(target)?).await?;
            println!("daemon stopped");
            Ok(())
        }
        DaemonCommand::Kill { session } => {
            goat_client::kill_session(&remote::resolve(target)?, session).await?;
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
        .devices
        .bind
        .parse()
        .map_err(|e| color_eyre::eyre::eyre!("invalid device bind address: {e}"))?;
    Ok(Some(goat_daemon::RemoteSettings {
        remote_dir,
        bind,
        advertised: config.devices.advertised,
    }))
}

async fn run_device_command(command: DeviceCommand) -> color_eyre::Result<()> {
    let link = remote::local()?;
    match command {
        DeviceCommand::Add { label } => {
            let label = label.unwrap_or_else(|| "device".to_owned());
            let info = goat_client::pair_device(&link, label).await?;
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
        DeviceCommand::List => {
            let devices = goat_client::list_devices(&link).await?;
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
        DeviceCommand::Remove { device } => {
            let ok = goat_client::revoke_device(&link, device.clone()).await?;
            if ok {
                println!("revoked device {device}");
            } else {
                println!("no such device: {device}");
            }
            Ok(())
        }
    }
}

async fn run_remote_command(command: RemoteCommand) -> color_eyre::Result<()> {
    let color = ColorMode::detect();
    match command {
        RemoteCommand::Add {
            name,
            host,
            fingerprint,
            code,
        } => {
            let promoted = remote::add(&name, &host, &fingerprint, &code).await?;
            ui::success(&format!("paired with {host} as `{name}`"));
            if promoted {
                println!(
                    "{}",
                    color.paint(
                        format!("`{name}` is now the default; `goat remote use local` goes back"),
                        Palette::Muted,
                    )
                );
            }
            Ok(())
        }
        RemoteCommand::List => {
            for row in remote::list() {
                let marker = if row.active { "*" } else { " " };
                println!(
                    "{} {} {}",
                    color.paint(marker, Palette::Success),
                    color.cell(row.name, Palette::Provider, 16),
                    color.paint(row.address, Palette::Value)
                );
            }
            Ok(())
        }
        RemoteCommand::Remove { name } => {
            if remote::remove(&name)? {
                println!("forgot remote {name}");
            } else {
                println!("no such remote: {name}");
            }
            Ok(())
        }
        RemoteCommand::Use { name } => {
            remote::select(&name)?;
            ui::success(&format!("`{name}` is now the default"));
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
