use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::config::{project_identity, write_atomic};
use crate::{McpError, ServerConfig};

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(transparent)]
struct ApprovalData(BTreeMap<String, BTreeMap<String, String>>);

pub struct Approvals {
    path: PathBuf,
    data: ApprovalData,
    original: Option<Vec<u8>>,
}

impl Approvals {
    pub fn load(path: PathBuf) -> Result<Self, McpError> {
        let (data, original) = match fs::read(&path) {
            Ok(raw) => (serde_json::from_slice(&raw)?, Some(raw)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                (ApprovalData::default(), None)
            }
            Err(error) => return Err(error.into()),
        };
        Ok(Self {
            path,
            data,
            original,
        })
    }

    pub fn approved(&self, project_root: &Path, name: &str, config: &ServerConfig) -> bool {
        self.data
            .0
            .get(&project_identity(project_root))
            .and_then(|servers| servers.get(name))
            .is_some_and(|approved| approved == &fingerprint(config))
    }

    pub fn approve(
        &mut self,
        project_root: &Path,
        name: &str,
        config: &ServerConfig,
    ) -> Result<(), McpError> {
        self.data
            .0
            .entry(project_identity(project_root))
            .or_default()
            .insert(name.to_owned(), fingerprint(config));
        self.save()
    }

    pub fn revoke(&mut self, project_root: &Path, name: &str) -> Result<(), McpError> {
        if let Some(servers) = self.data.0.get_mut(&project_identity(project_root)) {
            servers.remove(name);
        }
        self.save()
    }

    fn save(&mut self) -> Result<(), McpError> {
        let current = match fs::read(&self.path) {
            Ok(raw) => Some(raw),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(error.into()),
        };
        if current != self.original {
            return Err(McpError::Config(format!(
                "{} changed while it was being edited",
                self.path.display()
            )));
        }
        let mut raw = serde_json::to_vec_pretty(&self.data)?;
        raw.push(b'\n');
        write_atomic(&self.path, &raw)?;
        self.original = Some(raw);
        Ok(())
    }
}

fn fingerprint(config: &ServerConfig) -> String {
    use std::fmt::Write as _;

    let raw = serde_json::to_vec(config).unwrap_or_default();
    let digest = Sha256::digest(raw);
    let mut encoded = String::with_capacity(digest.len() * 2);
    for byte in digest {
        let _ = write!(encoded, "{byte:02x}");
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::StdioConfig;

    #[test]
    fn approval_is_bound_to_the_exact_server_config() {
        let dir = tempfile::tempdir().unwrap();
        let mut approvals = Approvals::load(dir.path().join("approvals.json")).unwrap();
        let mut server = ServerConfig::Stdio(StdioConfig {
            command: "one".to_owned(),
            args: Vec::new(),
            env: BTreeMap::new(),
        });
        approvals.approve(dir.path(), "server", &server).unwrap();
        assert!(approvals.approved(dir.path(), "server", &server));
        let ServerConfig::Stdio(stdio) = &mut server else {
            panic!("stdio")
        };
        stdio.command = "two".to_owned();
        assert!(!approvals.approved(dir.path(), "server", &server));
    }
}
