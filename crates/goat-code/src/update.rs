use std::{
    collections::HashMap,
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
};

use color_eyre::eyre::{Context, eyre};
use flate2::read::GzDecoder;
use semver::Version;
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::ui::{ColorMode, Palette, pair, pair_styled};

const REPOSITORY: &str = "goat-agent/goat";
const INSTALL_URL: &str = "https://raw.githubusercontent.com/goat-agent/goat/main/install.sh";

#[derive(Debug, Deserialize)]
struct Release {
    tag_name: String,
    assets: Vec<Asset>,
}

#[derive(Debug, Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
}

pub async fn run(force: bool) -> color_eyre::Result<()> {
    let target = target_triple()?;
    let current = Version::parse(env!("CARGO_PKG_VERSION"))?;
    let client = reqwest::Client::builder()
        .user_agent(format!("goat/{current}"))
        .build()?;
    let release = fetch_latest_release(&client).await?;
    let latest = parse_tag(&release.tag_name)?;

    if latest <= current && !force {
        let color = ColorMode::detect();
        println!("{}", color.paint("goat is up to date", Palette::Success));
        pair("current", &current.to_string());
        pair("latest", &latest.to_string());
        return Ok(());
    }

    drain_daemon(force).await?;

    let archive_name = format!("goat-{target}.tar.gz");
    let archive_url = asset_url(&release, &archive_name)?;
    let checksums_url = asset_url(&release, "SHA256SUMS")?;
    let color = ColorMode::detect();
    println!("{}", color.paint("updating goat", Palette::Provider));
    pair("current", &current.to_string());
    pair("latest", &latest.to_string());
    pair("target", target);
    let archive = download(&client, archive_url).await?;
    let checksums = String::from_utf8(download(&client, checksums_url).await?)?;
    verify_checksum(&archive_name, &archive, &checksums)?;
    pair_styled("checksum", "verified", Palette::Success);

    let staged_dir = stage_dir(&latest, target)?;
    reset_dir(&staged_dir)?;
    extract_archive(&archive, &staged_dir)?;
    let staged_bin = staged_dir.join(exe_name("goat"));
    require_file(&staged_bin)?;

    let bin_path = install_bin_path()?;
    pair("install", &bin_path.display().to_string());
    replace_binary(&bin_path, &staged_bin)?;
    pair_styled(
        "installed",
        "restart goat to run the new version",
        Palette::Success,
    );
    Ok(())
}

async fn drain_daemon(force: bool) -> color_eyre::Result<()> {
    let Some(socket) = goat_config::socket_path() else {
        return Ok(());
    };
    if !socket.exists() {
        return Ok(());
    }
    let Ok(sessions) = goat_client::status(&socket).await else {
        return Ok(());
    };
    let active = sessions
        .iter()
        .filter(|s| {
            matches!(
                s.state,
                goat_wire::SessionLiveState::Active {}
                    | goat_wire::SessionLiveState::WaitingOnAsk {}
            )
        })
        .count();
    if active > 0 && !force {
        return Err(eyre!(
            "{active} session(s) are still running in the daemon. Finish them or run `goat daemon stop`, then retry (or use `goat update --force`)."
        ));
    }
    println!("Stopping the running daemon before update...");
    let _ = goat_client::stop(&socket).await;
    Ok(())
}

async fn fetch_latest_release(client: &reqwest::Client) -> color_eyre::Result<Release> {
    let response = client
        .get(format!(
            "https://api.github.com/repos/{REPOSITORY}/releases/latest"
        ))
        .send()
        .await?
        .error_for_status()?;
    Ok(response.json().await?)
}

async fn download(client: &reqwest::Client, url: &str) -> color_eyre::Result<Vec<u8>> {
    let response = client.get(url).send().await?.error_for_status()?;
    Ok(response.bytes().await?.to_vec())
}

fn asset_url<'a>(release: &'a Release, name: &str) -> color_eyre::Result<&'a str> {
    release
        .assets
        .iter()
        .find(|asset| asset.name == name)
        .map(|asset| asset.browser_download_url.as_str())
        .ok_or_else(|| eyre!("release asset not found: {name}"))
}

fn parse_tag(tag: &str) -> color_eyre::Result<Version> {
    Ok(Version::parse(tag.strip_prefix('v').unwrap_or(tag))?)
}

fn target_triple() -> color_eyre::Result<&'static str> {
    if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        Ok("x86_64-unknown-linux-gnu")
    } else if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
        Ok("aarch64-unknown-linux-gnu")
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        Ok("x86_64-apple-darwin")
    } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        Ok("aarch64-apple-darwin")
    } else if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
        Ok("x86_64-pc-windows-msvc")
    } else {
        Err(eyre!("unsupported update target"))
    }
}

fn verify_checksum(name: &str, bytes: &[u8], checksums: &str) -> color_eyre::Result<()> {
    let expected = parse_checksums(checksums)
        .remove(name)
        .ok_or_else(|| eyre!("checksum not found for {name}"))?;
    let actual = hex_digest(bytes);
    if actual == expected {
        Ok(())
    } else {
        Err(eyre!("checksum mismatch for {name}"))
    }
}

fn parse_checksums(raw: &str) -> HashMap<String, String> {
    raw.lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let hash = parts.next()?;
            let name = parts.next()?.trim_start_matches('*');
            Some((name.to_string(), hash.to_ascii_lowercase()))
        })
        .collect()
}

fn hex_digest(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(out, "{byte:02x}");
    }
    out
}

fn stage_dir(version: &Version, target: &str) -> color_eyre::Result<PathBuf> {
    let base = goat_config::update_dir().ok_or_else(|| eyre!(goat_config::HOME_NOT_FOUND))?;
    Ok(base.join(format!("{version}-{target}")))
}

fn reset_dir(path: &Path) -> color_eyre::Result<()> {
    if path.exists() {
        fs::remove_dir_all(path)?;
    }
    fs::create_dir_all(path)?;
    Ok(())
}

fn extract_archive(bytes: &[u8], dest: &Path) -> color_eyre::Result<()> {
    let decoder = GzDecoder::new(bytes);
    let mut archive = tar::Archive::new(decoder);
    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?;
        let file_name = path
            .file_name()
            .ok_or_else(|| eyre!("archive entry has no file name"))?;
        let out = dest.join(file_name);
        entry.unpack(out)?;
    }
    Ok(())
}

fn install_bin_path() -> color_eyre::Result<PathBuf> {
    let current = std::env::current_exe()?;
    if let Some(expected) = goat_config::bin_dir()
        && current.parent() != Some(expected.as_path())
    {
        println!("Reinstall goat with:");
        println!("  curl -fsSL {INSTALL_URL} | sh");
        return Err(eyre!("goat is not installed in {}", expected.display()));
    }
    Ok(current)
}

fn replace_binary(bin_path: &Path, staged: &Path) -> color_eyre::Result<()> {
    match self_replace::self_replace(staged) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == ErrorKind::PermissionDenied => {
            Err(eyre!("permission denied writing {}", bin_path.display()))
        }
        Err(err) => Err(err).with_context(|| format!("replacing {}", bin_path.display())),
    }
}

fn require_file(path: &Path) -> color_eyre::Result<()> {
    if path.is_file() {
        Ok(())
    } else {
        Err(eyre!("required file is missing: {}", path.display()))
    }
}

fn exe_name(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::{hex_digest, parse_checksums, parse_tag};

    #[test]
    fn parses_v_tag() {
        assert_eq!(parse_tag("v1.2.3").unwrap().to_string(), "1.2.3");
    }

    #[test]
    fn parses_checksums() {
        let parsed = parse_checksums("abc  goat-code.tar.gz\ndef *other.tar.gz\n");
        assert_eq!(parsed["goat-code.tar.gz"], "abc");
        assert_eq!(parsed["other.tar.gz"], "def");
    }

    #[test]
    fn hashes_bytes() {
        assert_eq!(
            hex_digest(b"goat"),
            "5480f08f35968440ebe8135a8bf9e58c8c944bf4e3ba0f45acb141e474bd0c9c"
        );
    }
}
