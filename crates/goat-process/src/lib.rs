use std::io;

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
    use super::{KillError, kill_group};

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
