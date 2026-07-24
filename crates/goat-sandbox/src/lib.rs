use std::{ffi::OsString, path::Path};

#[derive(Debug, thiserror::Error)]
pub enum SandboxError {
    #[error("no sandbox backend is available on this platform")]
    Unavailable,
}

pub struct SandboxedCommand {
    pub program: OsString,
    pub args: Vec<OsString>,
}

pub fn backend_available() -> bool {
    backend::available()
}

pub fn read_only_command(
    command: &str,
    cwd: &Path,
    writable_tmp: &Path,
    network: bool,
) -> Result<SandboxedCommand, SandboxError> {
    if !backend::available() {
        return Err(SandboxError::Unavailable);
    }
    Ok(backend::read_only(command, cwd, writable_tmp, network))
}

#[cfg(target_os = "macos")]
mod backend {
    use super::SandboxedCommand;
    use std::{ffi::OsString, path::Path};

    const SANDBOX_EXEC: &str = "/usr/bin/sandbox-exec";

    const PROFILE_HEAD: &str = "(version 1)\n\
(deny default)\n\
(allow process-fork)\n\
(allow process-exec*)\n\
(allow signal)\n\
(allow sysctl-read)\n\
(allow mach-lookup)\n\
(allow ipc-posix-shm)\n\
(allow file-read*)\n\
(allow file-ioctl)\n";

    const PROFILE_WRITE: &str = "(allow file-write* (subpath (param \"GOAT_TMP\")))\n\
(allow file-write* (subpath \"/private/tmp\"))\n\
(allow file-write* (subpath \"/private/var/tmp\"))\n\
(allow file-write* (literal \"/dev/null\") (literal \"/dev/zero\") (literal \"/dev/random\") (literal \"/dev/urandom\") (literal \"/dev/tty\") (literal \"/dev/stdin\") (literal \"/dev/stdout\") (literal \"/dev/stderr\"))\n";

    const PROFILE_NET: &str = "(allow network*)\n";

    const SECRET_SUBDIRS: [&str; 9] = [
        ".ssh",
        ".goat",
        ".aws",
        ".gnupg",
        ".config/gcloud",
        ".config/goat-code",
        ".config/gh",
        ".kube",
        ".docker",
    ];

    const SECRET_FILES: [&str; 5] = [
        ".netrc",
        ".git-credentials",
        ".npmrc",
        ".pgpass",
        ".config/git/credentials",
    ];

    fn secret_read_denies() -> String {
        use std::fmt::Write as _;
        let Some(home) = std::env::var_os("HOME") else {
            return String::new();
        };
        let home = Path::new(&home);
        let mut out = String::new();
        for sub in SECRET_SUBDIRS {
            if let Some(path) = home.join(sub).to_str() {
                let escaped = path.replace('\\', "\\\\").replace('"', "\\\"");
                let _ = writeln!(out, "(deny file-read* (subpath \"{escaped}\"))");
            }
        }
        for sub in SECRET_FILES {
            if let Some(path) = home.join(sub).to_str() {
                let escaped = path.replace('\\', "\\\\").replace('"', "\\\"");
                let _ = writeln!(out, "(deny file-read* (literal \"{escaped}\"))");
            }
        }
        out
    }

    pub fn available() -> bool {
        Path::new(SANDBOX_EXEC).exists()
    }

    pub fn read_only(
        command: &str,
        _cwd: &Path,
        writable_tmp: &Path,
        network: bool,
    ) -> SandboxedCommand {
        let mut profile = String::with_capacity(512);
        profile.push_str(PROFILE_HEAD);
        profile.push_str(&secret_read_denies());
        if network {
            profile.push_str(PROFILE_NET);
        }
        profile.push_str(PROFILE_WRITE);
        let mut tmp_param = OsString::from("GOAT_TMP=");
        tmp_param.push(writable_tmp);
        let args = vec![
            OsString::from("-D"),
            tmp_param,
            OsString::from("-p"),
            OsString::from(profile),
            OsString::from("--"),
            OsString::from("sh"),
            OsString::from("-c"),
            OsString::from(command),
        ];
        SandboxedCommand {
            program: OsString::from(SANDBOX_EXEC),
            args,
        }
    }
}

#[cfg(target_os = "linux")]
mod backend {
    use super::SandboxedCommand;
    use std::{ffi::OsString, path::Path, sync::OnceLock};

    const SECRET_SUBDIRS: [&str; 9] = [
        ".ssh",
        ".goat",
        ".aws",
        ".gnupg",
        ".config/gcloud",
        ".config/goat-code",
        ".config/gh",
        ".kube",
        ".docker",
    ];

    const SECRET_FILES: [&str; 5] = [
        ".netrc",
        ".git-credentials",
        ".npmrc",
        ".pgpass",
        ".config/git/credentials",
    ];

    fn bwrap_path() -> Option<&'static Path> {
        static PATH: OnceLock<Option<std::path::PathBuf>> = OnceLock::new();
        PATH.get_or_init(|| {
            std::env::var_os("PATH").and_then(|paths| {
                std::env::split_paths(&paths)
                    .map(|dir| dir.join("bwrap"))
                    .find(|candidate| candidate.exists())
            })
        })
        .as_deref()
    }

    pub fn available() -> bool {
        bwrap_path().is_some()
    }

    pub fn read_only(
        command: &str,
        cwd: &Path,
        writable_tmp: &Path,
        network: bool,
    ) -> SandboxedCommand {
        let bwrap =
            bwrap_path().map_or_else(|| OsString::from("bwrap"), |p| p.as_os_str().to_owned());
        let mut args: Vec<OsString> = vec![
            OsString::from("--ro-bind"),
            OsString::from("/"),
            OsString::from("/"),
            OsString::from("--dev"),
            OsString::from("/dev"),
            OsString::from("--proc"),
            OsString::from("/proc"),
            OsString::from("--bind"),
            writable_tmp.into(),
            writable_tmp.into(),
            OsString::from("--unshare-user"),
            OsString::from("--unshare-pid"),
            OsString::from("--unshare-ipc"),
            OsString::from("--unshare-uts"),
            OsString::from("--unshare-cgroup-try"),
            OsString::from("--die-with-parent"),
        ];
        for sub in SECRET_SUBDIRS {
            if let Some(home) = std::env::var_os("HOME") {
                let path = Path::new(&home).join(sub);
                if path.exists() {
                    args.push(OsString::from("--tmpfs"));
                    args.push(path.into());
                }
            }
        }
        for sub in SECRET_FILES {
            if let Some(home) = std::env::var_os("HOME") {
                let path = Path::new(&home).join(sub);
                if path.exists() {
                    args.push(OsString::from("--ro-bind"));
                    args.push(OsString::from("/dev/null"));
                    args.push(path.into());
                }
            }
        }
        args.push(OsString::from("--chdir"));
        args.push(cwd.into());
        if !network {
            args.push(OsString::from("--unshare-net"));
        }
        args.push(OsString::from("sh"));
        args.push(OsString::from("-c"));
        args.push(OsString::from(command));
        SandboxedCommand {
            program: bwrap,
            args,
        }
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
mod backend {
    use super::SandboxedCommand;
    use std::path::Path;

    pub fn available() -> bool {
        false
    }

    pub fn read_only(
        _command: &str,
        _cwd: &Path,
        _writable_tmp: &Path,
        _network: bool,
    ) -> SandboxedCommand {
        unreachable!("read_only is gated by backend_available()")
    }
}

#[cfg(test)]
#[cfg(target_os = "macos")]
mod tests {
    use std::path::Path;

    #[test]
    fn read_only_profile_denies_secret_paths_but_allows_general_reads() {
        let home = std::env::var("HOME").expect("HOME");
        let cmd =
            super::backend::read_only("echo hi", Path::new("/work"), Path::new("/tmp/g"), false);
        let profile = cmd
            .args
            .iter()
            .filter_map(|a| a.to_str())
            .find(|a| a.contains("(deny default)"))
            .expect("profile arg");
        assert!(profile.contains("(allow file-read*)"));
        assert!(profile.contains(&format!("(deny file-read* (subpath \"{home}/.ssh\"))")));
        assert!(profile.contains(&format!("(deny file-read* (subpath \"{home}/.goat\"))")));
        assert!(profile.contains(&format!("(deny file-read* (literal \"{home}/.netrc\"))")));
        assert!(!profile.contains("/work"));
    }
}
