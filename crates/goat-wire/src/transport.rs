use std::path::Path;

#[cfg(unix)]
pub type Stream = tokio::net::UnixStream;
#[cfg(not(unix))]
pub type Stream = tokio::net::TcpStream;

pub struct Listener {
    #[cfg(unix)]
    inner: tokio::net::UnixListener,
    #[cfg(not(unix))]
    inner: tokio::net::TcpListener,
    #[cfg(not(unix))]
    port_file: std::path::PathBuf,
}

impl Listener {
    pub async fn accept(&self) -> std::io::Result<Stream> {
        let (stream, _addr) = self.inner.accept().await?;
        Ok(stream)
    }
}

#[cfg(unix)]
pub fn bind(path: &Path) -> std::io::Result<Listener> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if path.exists() {
        let _ = std::fs::remove_file(path);
    }
    let inner = tokio::net::UnixListener::bind(path)?;
    set_permissions(path)?;
    Ok(Listener { inner })
}

#[cfg(not(unix))]
pub fn bind(path: &Path) -> std::io::Result<Listener> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let inner = std::net::TcpListener::bind(("127.0.0.1", 0))?;
    inner.set_nonblocking(true)?;
    let port = inner.local_addr()?.port();
    let inner = tokio::net::TcpListener::from_std(inner)?;
    let port_file = port_file(path);
    std::fs::write(&port_file, port.to_string())?;
    Ok(Listener { inner, port_file })
}

#[cfg(unix)]
pub async fn connect(path: &Path) -> std::io::Result<Stream> {
    tokio::net::UnixStream::connect(path).await
}

#[cfg(not(unix))]
pub async fn connect(path: &Path) -> std::io::Result<Stream> {
    let port = read_port(path)?;
    tokio::net::TcpStream::connect(("127.0.0.1", port)).await
}

#[cfg(unix)]
pub fn probe_alive(path: &Path) -> bool {
    std::os::unix::net::UnixStream::connect(path).is_ok()
}

#[cfg(not(unix))]
pub fn probe_alive(path: &Path) -> bool {
    read_port(path)
        .and_then(|port| std::net::TcpStream::connect(("127.0.0.1", port)))
        .is_ok()
}

#[cfg(unix)]
pub fn cleanup(path: &Path) {
    let _ = std::fs::remove_file(path);
}

#[cfg(not(unix))]
pub fn cleanup(path: &Path) {
    let _ = std::fs::remove_file(port_file(path));
}

pub fn exists(path: &Path) -> bool {
    #[cfg(unix)]
    {
        path.exists()
    }
    #[cfg(not(unix))]
    {
        port_file(path).exists()
    }
}

#[cfg(unix)]
fn set_permissions(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
}

#[cfg(not(unix))]
fn port_file(path: &Path) -> std::path::PathBuf {
    path.with_extension("port")
}

#[cfg(not(unix))]
fn read_port(path: &Path) -> std::io::Result<u16> {
    let raw = std::fs::read_to_string(port_file(path))?;
    raw.trim()
        .parse::<u16>()
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidData, "bad port file"))
}

#[cfg(not(unix))]
impl Drop for Listener {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.port_file);
    }
}
