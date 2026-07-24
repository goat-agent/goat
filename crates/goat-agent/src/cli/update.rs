use std::collections::HashMap;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use clap::Args as ClapArgs;
use flate2::read::GzDecoder;
use goat_config::GoatPaths;
use semver::Version;
use serde::Deserialize;
use sha2::{Digest, Sha256};

use super::ui::{self, Footer};

const REPOSITORY: &str = "goat-agent/goat-agent";

#[derive(ClapArgs, Debug, Default)]
pub struct Args {
    #[arg(
        long,
        help = "Reinstall the latest release even if the current version is already up to date."
    )]
    pub force: bool,
}

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

pub async fn run(args: Args) -> Result<()> {
    let target = target_triple()?;
    let current = Version::parse(env!("CARGO_PKG_VERSION"))?;
    let client = reqwest::Client::builder()
        .user_agent(format!("goat-agent/{current}"))
        .build()?;

    let release = fetch_latest_release(&client).await?;
    let latest = parse_tag(&release.tag_name)?;

    if latest <= current && !args.force {
        return ui::cell("Update", || {
            ui::pair("current", &current.to_string());
            ui::pair("latest", &latest.to_string());
            Ok(Footer::Ok("already up to date"))
        });
    }

    let archive_name = format!("goat-agent-{target}.tar.gz");
    let archive_url = asset_url(&release, &archive_name)?;
    let checksums_url = asset_url(&release, "SHA256SUMS")?;

    let archive = download(&client, archive_url).await?;
    let checksums = String::from_utf8(download(&client, checksums_url).await?)
        .context("SHA256SUMS is not valid UTF-8")?;
    verify_checksum(&archive_name, &archive, &checksums)?;

    let bin_path = std::env::current_exe().context("resolving current executable")?;
    let staged = extract_binary(&archive, target)?;
    replace_binary(&bin_path, &staged)?;

    ui::cell("Update", || {
        ui::pair("current", &current.to_string());
        ui::pair("latest", &latest.to_string());
        ui::pair("target", target);
        ui::pair("checksum", "verified");
        ui::pair("installed", &bin_path.display().to_string());
        Ok(Footer::Ok("updated — restart goat to run the new version"))
    })
}

async fn fetch_latest_release(client: &reqwest::Client) -> Result<Release> {
    let response = client
        .get(format!(
            "https://api.github.com/repos/{REPOSITORY}/releases/latest"
        ))
        .send()
        .await?
        .error_for_status()
        .context("fetching latest release")?;
    response.json().await.context("parsing release metadata")
}

async fn download(client: &reqwest::Client, url: &str) -> Result<Vec<u8>> {
    let response = client
        .get(url)
        .send()
        .await?
        .error_for_status()
        .with_context(|| format!("downloading {url}"))?;
    Ok(response.bytes().await?.to_vec())
}

fn asset_url<'a>(release: &'a Release, name: &str) -> Result<&'a str> {
    release
        .assets
        .iter()
        .find(|asset| asset.name == name)
        .map(|asset| asset.browser_download_url.as_str())
        .ok_or_else(|| anyhow!("release asset not found: {name}"))
}

fn parse_tag(tag: &str) -> Result<Version> {
    Version::parse(tag.strip_prefix('v').unwrap_or(tag))
        .with_context(|| format!("parsing release tag: {tag}"))
}

fn target_triple() -> Result<&'static str> {
    if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        Ok("x86_64-unknown-linux-gnu")
    } else if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
        Ok("aarch64-unknown-linux-gnu")
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        Ok("x86_64-apple-darwin")
    } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        Ok("aarch64-apple-darwin")
    } else {
        Err(anyhow!("no prebuilt release for this platform"))
    }
}

fn verify_checksum(name: &str, bytes: &[u8], checksums: &str) -> Result<()> {
    let expected = parse_checksums(checksums)
        .remove(name)
        .ok_or_else(|| anyhow!("checksum not found for {name}"))?;
    let actual = hex_digest(bytes);
    if actual == expected {
        Ok(())
    } else {
        Err(anyhow!("checksum mismatch for {name}"))
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

fn extract_binary(bytes: &[u8], target: &str) -> Result<PathBuf> {
    let bin_name = exe_name("goat-agent");
    let dir = staging_dir()?;
    if dir.exists() {
        std::fs::remove_dir_all(&dir).with_context(|| format!("clearing {}", dir.display()))?;
    }
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;

    let decoder = GzDecoder::new(bytes);
    let mut archive = tar::Archive::new(decoder);
    for entry in archive.entries().context("reading archive")? {
        let mut entry = entry?;
        let path = entry.path()?;
        let Some(file_name) = path.file_name() else {
            continue;
        };
        if file_name == bin_name.as_str() {
            let out = dir.join(&bin_name);
            entry
                .unpack(&out)
                .with_context(|| format!("unpacking {}", out.display()))?;
            return Ok(out);
        }
    }
    Err(anyhow!(
        "archive goat-agent-{target}.tar.gz did not contain a `{bin_name}` binary"
    ))
}

fn replace_binary(bin_path: &Path, staged: &Path) -> Result<()> {
    match self_replace::self_replace(staged) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == ErrorKind::PermissionDenied => Err(anyhow!(
            "permission denied writing {}. Re-run with elevated privileges, e.g. `sudo goat update`.",
            bin_path.display()
        )),
        Err(err) => Err(err).with_context(|| format!("replacing {}", bin_path.display())),
    }
}

fn staging_dir() -> Result<PathBuf> {
    Ok(GoatPaths::default_layout()?.root.join("update"))
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
        assert_eq!(parse_tag("0.4.1").unwrap().to_string(), "0.4.1");
    }

    #[test]
    fn parses_checksums() {
        let parsed = parse_checksums("abc  goat-x86_64.tar.gz\nDEF *other.tar.gz\n");
        assert_eq!(parsed["goat-x86_64.tar.gz"], "abc");
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
