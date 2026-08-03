use std::path::Path;

pub type Stream = tokio::net::UnixStream;

pub struct Listener {
    inner: tokio::net::UnixListener,
}

impl Listener {
    pub async fn accept(&self) -> std::io::Result<Stream> {
        let (stream, _addr) = self.inner.accept().await?;
        Ok(stream)
    }
}

pub fn bind(path: &Path) -> std::io::Result<Listener> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let inner = tokio::net::UnixListener::bind(path)?;
    set_permissions(path)?;
    Ok(Listener { inner })
}

pub async fn connect(path: &Path) -> std::io::Result<Stream> {
    tokio::net::UnixStream::connect(path).await
}

pub fn cleanup(path: &Path) {
    let _ = std::fs::remove_file(path);
}

fn set_permissions(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
}
