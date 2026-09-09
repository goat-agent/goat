use std::{collections::HashMap, path::PathBuf, sync::Arc};

use goat_api::{
    AgentChat, AgentChatParams, AgentList, AgentListOutput, AgentSchedules, AgentSchedulesOutput,
    AgentSchedulesParams, AgentSend, AgentSendOutput, AgentSendParams, AgentWatch,
    AgentWatchParams, ConversationInfo, ConversationList, ConversationListParams, DaemonStatus,
    DaemonStatus2, Empty, StreamEvent, WatchFrom,
};
use goat_client::{ApiSession, Link};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use tauri::{AppHandle, Emitter, State};
use tokio::{sync::Mutex, task::JoinHandle};

pub struct Backend {
    link: Arc<Link>,
    connection: Mutex<Option<Arc<ApiSession>>>,
    streams: parking_lot::Mutex<HashMap<String, JoinHandle<()>>>,
    projects: Arc<parking_lot::Mutex<()>>,
}

impl Backend {
    pub fn new(link: Arc<Link>) -> Self {
        Self {
            link,
            connection: Mutex::new(None),
            streams: parking_lot::Mutex::new(HashMap::new()),
            projects: Arc::new(parking_lot::Mutex::new(())),
        }
    }

    pub async fn connection(&self) -> Result<Arc<ApiSession>, String> {
        let mut connection = self.connection.lock().await;
        if let Some(session) = connection
            .as_ref()
            .filter(|session| !session.closed.is_cancelled())
        {
            return Ok(session.clone());
        }
        let Link::Local {
            socket_path,
            daemon_exe,
        } = self.link.as_ref()
        else {
            return Err("goat-desktop connects only to the local daemon".into());
        };
        let session = match goat_client::open_api(&self.link, "goat-desktop").await {
            Ok(session) => session,
            Err(goat_client::ClientError::Io(error))
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
                ) =>
            {
                goat_client::start(socket_path, daemon_exe)
                    .await
                    .map_err(|error| error.to_string())?;
                goat_client::open_api(&self.link, "goat-desktop")
                    .await
                    .map_err(|error| error.to_string())?
            }
            Err(error) => return Err(error.to_string()),
        };
        let session = Arc::new(session);
        *connection = Some(session.clone());
        Ok(session)
    }

    fn stream<T: Serialize + DeserializeOwned + Send + 'static>(
        &self,
        mut stream: goat_api::Stream<T>,
        session: Arc<ApiSession>,
        app: AppHandle,
        event: String,
    ) {
        let key = event.clone();
        let task = tokio::spawn(async move {
            let _session = session;
            let mut error = None;
            while let Some(item) = stream.recv().await {
                match item {
                    StreamEvent::Item { item, .. } => {
                        if let Err(failure) = app.emit(&event, &item) {
                            error = Some(failure.to_string());
                            break;
                        }
                    }
                    StreamEvent::End(result) => {
                        error = result.err().map(|error| error.to_string());
                        break;
                    }
                }
            }
            if let Some(error) = &error {
                tracing::warn!(%event, %error, "desktop stream ended");
            }
            let _ = app.emit(&format!("{event}:closed"), error);
        });
        if let Some(previous) = self.streams.lock().insert(key, task) {
            previous.abort();
        }
    }

    fn close_stream(&self, event: &str) {
        if let Some(task) = self.streams.lock().remove(event) {
            task.abort();
        }
    }
}

impl Drop for Backend {
    fn drop(&mut self) {
        for (_, task) in self.streams.get_mut().drain() {
            task.abort();
        }
        if let Some(session) = self.connection.get_mut().take() {
            session.shutdown();
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Project {
    pub path: String,
    pub added_at: String,
}

#[derive(Default, Serialize, Deserialize)]
struct DesktopConfig {
    projects: Vec<Project>,
}

fn project_config() -> Result<(PathBuf, DesktopConfig), String> {
    let path = goat_config::desktop_path().ok_or(goat_config::HOME_NOT_FOUND)?;
    let config = match std::fs::read(&path) {
        Ok(bytes) => {
            serde_json::from_slice(&bytes).map_err(|error| format!("desktop config: {error}"))?
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => DesktopConfig::default(),
        Err(error) => return Err(error.to_string()),
    };
    Ok((path, config))
}

#[tauri::command]
pub async fn projects_list(backend: State<'_, Backend>) -> Result<Vec<Project>, String> {
    let lock = backend.projects.clone();
    tokio::task::spawn_blocking(move || {
        let _lock = lock.lock();
        Ok(project_config()?.1.projects)
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub async fn projects_add(
    backend: State<'_, Backend>,
    path: String,
) -> Result<Vec<Project>, String> {
    let lock = backend.projects.clone();
    tokio::task::spawn_blocking(move || {
        let directory = PathBuf::from(path);
        if !directory.is_dir() {
            return Err("not a directory".into());
        }
        let path = directory
            .canonicalize()
            .map_err(|error| error.to_string())?
            .to_string_lossy()
            .into_owned();
        let _lock = lock.lock();
        let (file, mut config) = project_config()?;
        if !config.projects.iter().any(|project| project.path == path) {
            config.projects.push(Project {
                path,
                added_at: chrono::Utc::now().to_rfc3339(),
            });
            goat_config::write_atomic(
                &file,
                &serde_json::to_vec_pretty(&config).map_err(|error| error.to_string())?,
            )
            .map_err(|error| error.to_string())?;
        }
        Ok(config.projects)
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub async fn projects_remove(
    backend: State<'_, Backend>,
    path: String,
) -> Result<Vec<Project>, String> {
    let lock = backend.projects.clone();
    tokio::task::spawn_blocking(move || {
        let _lock = lock.lock();
        let (file, mut config) = project_config()?;
        config.projects.retain(|project| project.path != path);
        goat_config::write_atomic(
            &file,
            &serde_json::to_vec_pretty(&config).map_err(|error| error.to_string())?,
        )
        .map_err(|error| error.to_string())?;
        Ok(config.projects)
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub async fn daemon_status(backend: State<'_, Backend>) -> Result<DaemonStatus2, String> {
    backend
        .connection()
        .await?
        .api
        .call::<DaemonStatus>(Empty {})
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn conversations(
    backend: State<'_, Backend>,
    cwd: String,
) -> Result<Vec<ConversationInfo>, String> {
    Ok(backend
        .connection()
        .await?
        .api
        .call::<ConversationList>(ConversationListParams { cwd })
        .await
        .map_err(|error| error.to_string())?
        .conversations)
}

#[tauri::command]
pub async fn workspace(cwd: String) -> Result<serde_json::Value, String> {
    tokio::task::spawn_blocking(move || {
        let directory = PathBuf::from(cwd);
        if !directory.is_dir() { return Err("not a directory".into()); }
        let workspace = match goat_worktree::workspace(&directory) {
            Ok(workspace) => workspace,
            Err(goat_worktree::WorktreeError::Git(goat_git::GitError::NotARepository)) => {
                return Ok(serde_json::json!({"repo":null,"branch":null,"kind":"directory","pr":null}));
            }
            Err(error) => return Err(error.to_string()),
        };
        let pr = if goat_github::gh_available() { goat_github::pr_for_branch(&workspace.repo_root, &workspace.git_branch) } else { None };
        let pr = pr.map(|pr| serde_json::json!({"number":pr.number,"state":match pr.state { goat_github::PrState::Open => "open", goat_github::PrState::Merged => "merged", goat_github::PrState::Closed => "closed" }}));
        let kind = match workspace.kind { goat_worktree::WorkspaceKind::Main => "main", goat_worktree::WorkspaceKind::Managed { .. } => "managed", goat_worktree::WorkspaceKind::OtherWorktree => "worktree" };
        Ok(serde_json::json!({"repo":workspace.repo_root.to_string_lossy(),"branch":workspace.git_branch,"kind":kind,"pr":pr}))
    }).await.map_err(|error| error.to_string())?
}

#[tauri::command]
pub async fn agents(backend: State<'_, Backend>) -> Result<AgentListOutput, String> {
    backend
        .connection()
        .await?
        .api
        .call::<AgentList>(Empty {})
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn agent_send(
    backend: State<'_, Backend>,
    slug: String,
    text: String,
) -> Result<AgentSendOutput, String> {
    backend
        .connection()
        .await?
        .api
        .call::<AgentSend>(AgentSendParams { agent: slug, text })
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn agent_schedules(
    backend: State<'_, Backend>,
    slug: String,
) -> Result<AgentSchedulesOutput, String> {
    backend
        .connection()
        .await?
        .api
        .call::<AgentSchedules>(AgentSchedulesParams { agent: slug })
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn agent_chat_open(
    backend: State<'_, Backend>,
    app: AppHandle,
    slug: String,
) -> Result<(), String> {
    let connection = backend.connection().await?;
    let stream = connection
        .api
        .open::<AgentChat>(AgentChatParams {
            agent: slug.clone(),
            from: WatchFrom::Snapshot {},
        })
        .await
        .map_err(|error| error.to_string())?;
    backend.stream(stream, connection, app, format!("agent:{slug}"));
    Ok(())
}

#[tauri::command]
pub async fn agent_activity_open(
    backend: State<'_, Backend>,
    app: AppHandle,
    slug: String,
) -> Result<(), String> {
    let connection = backend.connection().await?;
    let stream = connection
        .api
        .open::<AgentWatch>(AgentWatchParams {
            agents: vec![slug.clone()],
            from: WatchFrom::Snapshot {},
        })
        .await
        .map_err(|error| error.to_string())?;
    backend.stream(stream, connection, app, format!("agent:{slug}:activity"));
    Ok(())
}

#[allow(clippy::needless_pass_by_value)]
#[tauri::command]
pub fn agent_chat_close(backend: State<'_, Backend>, slug: &str) {
    backend.close_stream(&format!("agent:{slug}"));
}

#[allow(clippy::needless_pass_by_value)]
#[tauri::command]
pub fn agent_activity_close(backend: State<'_, Backend>, slug: &str) {
    backend.close_stream(&format!("agent:{slug}:activity"));
}
