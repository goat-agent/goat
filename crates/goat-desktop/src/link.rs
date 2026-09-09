use std::sync::Arc;

pub fn local() -> Result<Arc<goat_client::Link>, String> {
    let paths = goat_config::GoatPaths::default_layout().map_err(|error| error.to_string())?;
    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
    let sibling = executable.with_file_name("goat");
    let daemon = if sibling.is_file() {
        sibling
    } else {
        paths.bin_dir.join("goat")
    };
    Ok(Arc::new(goat_client::Link::local(
        paths.socket_path,
        daemon,
    )))
}
