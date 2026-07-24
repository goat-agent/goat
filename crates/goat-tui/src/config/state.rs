use goat_protocol::{AuthMethod, LoginCredential};

pub(crate) const FIELD_LABEL_W: usize = 7;

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Section {
    Providers,
    Appearance,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Field {
    Name,
    Key,
}

pub(crate) enum InputStage {
    List,
    Choosing {
        provider: String,
        method: AuthMethod,
    },
    Adding {
        provider: String,
        method: AuthMethod,
        name: String,
        key: String,
        field: Field,
    },
    Waiting {
        provider: String,
        method: AuthMethod,
        name: String,
        status: Option<String>,
    },
}

pub enum StageKind {
    List,
    Input,
    Waiting,
}

pub enum ConfigOutcome {
    Pending,
    AddAccount {
        provider: String,
        name: String,
        credential: LoginCredential,
    },
    RemoveAccount {
        provider: String,
        name: String,
    },
    SetTheme {
        dark: bool,
    },
    SetMouseCapture {
        enabled: bool,
    },
    SetComputerUse {
        enabled: bool,
    },
    SetBrowser {
        enabled: bool,
    },
}

#[derive(Clone)]
pub(crate) struct Row {
    pub(crate) kind: RowKind,
    pub(crate) provider_index: usize,
    pub(crate) account_index: Option<usize>,
}

#[derive(Clone, PartialEq)]
pub(crate) enum RowKind {
    ProviderHeader,
    Account,
    AddAccount,
}
