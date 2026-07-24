#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum SandboxPolicy {
    #[default]
    Full,
    ReadOnly {
        network: bool,
    },
}

impl SandboxPolicy {
    pub fn is_read_only(&self) -> bool {
        matches!(self, Self::ReadOnly { .. })
    }
}
