use std::collections::HashSet;
use std::ffi::{OsStr, OsString};
use std::io;

const CHILD_ENV: [&str; 25] = [
    "APPDATA",
    "COLORTERM",
    "COMSPEC",
    "GOAT_CHILD_ENV",
    "GOAT_LOG",
    "HOME",
    "LANG",
    "LANGUAGE",
    "LOCALAPPDATA",
    "LOGNAME",
    "NO_COLOR",
    "PATH",
    "PATHEXT",
    "SystemRoot",
    "TEMP",
    "TERM",
    "TMP",
    "TMPDIR",
    "TZ",
    "USER",
    "USERNAME",
    "USERPROFILE",
    "XDG_CACHE_HOME",
    "XDG_CONFIG_HOME",
    "XDG_DATA_HOME",
];
const CHILD_ENV_EXTENSION: &str = "GOAT_CHILD_ENV";

pub fn child_environment() -> Vec<(OsString, OsString)> {
    child_environment_from(std::env::vars_os())
}

fn child_environment_from(
    source: impl IntoIterator<Item = (OsString, OsString)>,
) -> Vec<(OsString, OsString)> {
    let source: Vec<_> = source.into_iter().collect();
    let extensions = source
        .iter()
        .find(|(name, _)| name == CHILD_ENV_EXTENSION)
        .map(|(_, value)| extension_names(value))
        .unwrap_or_default();
    source
        .into_iter()
        .filter(|(name, _)| allowed_child_name(name, &extensions))
        .collect()
}

fn extension_names(raw: &OsStr) -> HashSet<String> {
    raw.to_string_lossy()
        .split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .collect()
}

fn allowed_child_name(name: &OsStr, extensions: &HashSet<String>) -> bool {
    let name = name.to_string_lossy();
    CHILD_ENV.contains(&name.as_ref())
        || name.starts_with("LC_")
        || extensions.contains(name.as_ref())
}

#[derive(Debug, thiserror::Error)]
pub enum KillError {
    #[error("refusing to signal process group {pgid}")]
    Refused { pgid: i32 },
    #[error(transparent)]
    Os(#[from] io::Error),
}

#[cfg(unix)]
pub fn kill_group(pgid: i32) -> Result<(), KillError> {
    use rustix::process::{Signal, kill_process_group};

    let group = checked_group(pgid)?;
    match kill_process_group(group, Signal::KILL) {
        Ok(()) | Err(rustix::io::Errno::SRCH) => Ok(()),
        Err(err) => Err(KillError::Os(err.into())),
    }
}

#[cfg(unix)]
pub fn group_is_alive(pgid: i32) -> bool {
    use rustix::process::{Pid, test_kill_process_group};

    let Some(group) = Pid::from_raw(pgid) else {
        return false;
    };
    !matches!(test_kill_process_group(group), Err(rustix::io::Errno::SRCH))
}

#[cfg(not(unix))]
pub fn group_is_alive(_pgid: i32) -> bool {
    false
}

#[cfg(unix)]
fn checked_group(pgid: i32) -> Result<rustix::process::Pid, KillError> {
    use rustix::process::{Pid, getpgrp};

    if pgid <= 1 {
        return Err(KillError::Refused { pgid });
    }
    let group = Pid::from_raw(pgid).ok_or(KillError::Refused { pgid })?;
    if group == getpgrp() {
        return Err(KillError::Refused { pgid });
    }
    Ok(group)
}

#[cfg(windows)]
pub fn kill_group(pgid: i32) -> Result<(), KillError> {
    if pgid <= 1 {
        return Err(KillError::Refused { pgid });
    }
    std::process::Command::new("taskkill")
        .args(["/F", "/T", "/PID"])
        .arg(pgid.to_string())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()?;
    Ok(())
}

#[cfg(not(any(unix, windows)))]
pub fn kill_group(pgid: i32) -> Result<(), KillError> {
    Err(KillError::Refused { pgid })
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::ffi::OsString;
    use std::process::Command;

    use super::{KillError, child_environment_from, kill_group};

    fn env_source(entries: &[(&str, &str)]) -> Vec<(OsString, OsString)> {
        entries
            .iter()
            .map(|(name, value)| (OsString::from(name), OsString::from(value)))
            .collect()
    }

    #[test]
    fn child_environment_keeps_runtime_values_and_drops_ambient_credentials() {
        let env = child_environment_from(env_source(&[
            ("PATH", "/usr/bin:/bin"),
            ("HOME", "/tmp/home"),
            ("LC_CTYPE", "UTF-8"),
            ("AWS_SECRET_ACCESS_KEY", "aws-secret"),
            ("GITHUB_TOKEN", "github-secret"),
            ("SSH_AUTH_SOCK", "/tmp/agent.sock"),
        ]));
        let names: HashSet<_> = env
            .iter()
            .map(|(name, _)| name.to_string_lossy().into_owned())
            .collect();
        assert!(names.contains("PATH"));
        assert!(names.contains("HOME"));
        assert!(names.contains("LC_CTYPE"));
        assert!(!names.contains("AWS_SECRET_ACCESS_KEY"));
        assert!(!names.contains("GITHUB_TOKEN"));
        assert!(!names.contains("SSH_AUTH_SOCK"));
    }

    #[test]
    fn child_environment_accepts_explicit_extensions() {
        let env = child_environment_from(env_source(&[
            ("GOAT_CHILD_ENV", "TOOL_LICENSE,SSH_AUTH_SOCK"),
            ("TOOL_LICENSE", "license"),
            ("SSH_AUTH_SOCK", "/tmp/agent.sock"),
            ("DATABASE_URL", "database-secret"),
        ]));
        let names: HashSet<_> = env
            .iter()
            .map(|(name, _)| name.to_string_lossy().into_owned())
            .collect();
        assert!(names.contains("TOOL_LICENSE"));
        assert!(names.contains("SSH_AUTH_SOCK"));
        assert!(!names.contains("DATABASE_URL"));
        assert!(names.contains("GOAT_CHILD_ENV"));
    }

    #[test]
    fn absent_explicit_extensions_remain_absent() {
        let env = child_environment_from(env_source(&[(
            "GOAT_CHILD_ENV",
            "SSH_AUTH_SOCK,TOOL_LICENSE",
        )]));
        let names: HashSet<_> = env
            .iter()
            .map(|(name, _)| name.to_string_lossy().into_owned())
            .collect();
        assert!(!names.contains("SSH_AUTH_SOCK"));
        assert!(!names.contains("TOOL_LICENSE"));
    }

    #[cfg(unix)]
    #[test]
    fn filtered_environment_is_the_environment_received_by_a_child() {
        let env = child_environment_from(env_source(&[
            ("PATH", "/usr/bin:/bin"),
            ("HOME", "/tmp"),
            ("AWS_SECRET_ACCESS_KEY", "not-for-child"),
        ]));
        let output = Command::new("/bin/sh")
            .arg("-c")
            .arg("command -v env && test -z \"$AWS_SECRET_ACCESS_KEY\"")
            .env("AWS_SECRET_ACCESS_KEY", "builder-secret")
            .env_clear()
            .envs(env)
            .output()
            .unwrap();
        assert!(output.status.success());
        assert!(!output.stdout.is_empty());
    }

    #[test]
    fn refuses_every_process() {
        assert!(matches!(
            kill_group(-1),
            Err(KillError::Refused { pgid: -1 })
        ));
    }

    #[test]
    fn refuses_own_process() {
        assert!(matches!(kill_group(0), Err(KillError::Refused { pgid: 0 })));
    }

    #[test]
    fn refuses_init() {
        assert!(matches!(kill_group(1), Err(KillError::Refused { pgid: 1 })));
    }

    #[cfg(unix)]
    #[test]
    fn refuses_own_group() {
        let own = rustix::process::getpgrp().as_raw_nonzero().get();
        assert!(matches!(kill_group(own), Err(KillError::Refused { .. })));
    }

    #[cfg(unix)]
    #[test]
    fn vanished_group_is_success() {
        assert!(kill_group(i32::MAX).is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn kills_a_real_group() {
        use std::os::unix::process::CommandExt;
        use std::process::{Command, Stdio};

        let mut child = Command::new("sleep")
            .arg("30")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(0)
            .spawn()
            .expect("spawn");

        let pgid = i32::try_from(child.id()).expect("pid fits");
        assert!(super::group_is_alive(pgid));
        kill_group(pgid).expect("kill the child group");

        let status = child.wait().expect("wait");
        assert!(!status.success(), "child should have been killed");
        assert!(!super::group_is_alive(pgid));
    }
}
