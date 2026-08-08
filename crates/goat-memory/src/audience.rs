#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Audience {
    kind: AudienceKind,
    reference: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AudienceKind {
    Global,
    Principal,
    Shared,
}

#[derive(Debug, thiserror::Error, Eq, PartialEq)]
pub enum AudienceError {
    #[error("empty audience reference")]
    Empty,
}

impl Audience {
    pub const fn global() -> Self {
        Self {
            kind: AudienceKind::Global,
            reference: None,
        }
    }

    pub fn principal(reference: impl Into<String>) -> Result<Self, AudienceError> {
        Self::identified(AudienceKind::Principal, reference)
    }

    pub fn shared(reference: impl Into<String>) -> Result<Self, AudienceError> {
        Self::identified(AudienceKind::Shared, reference)
    }

    fn identified(kind: AudienceKind, reference: impl Into<String>) -> Result<Self, AudienceError> {
        let reference = reference.into();
        if reference.is_empty() {
            Err(AudienceError::Empty)
        } else {
            Ok(Self {
                kind,
                reference: Some(reference),
            })
        }
    }

    pub(crate) const fn kind(&self) -> &'static str {
        match self.kind {
            AudienceKind::Global => "global",
            AudienceKind::Principal => "principal",
            AudienceKind::Shared => "shared",
        }
    }

    pub(crate) fn reference(&self) -> Option<&str> {
        self.reference.as_deref()
    }

    pub(crate) fn from_parts(kind: &str, reference: Option<String>) -> Option<Self> {
        match (kind, reference) {
            ("global", None) => Some(Self::global()),
            ("principal", Some(reference)) => Self::principal(reference).ok(),
            ("shared", Some(reference)) => Self::shared(reference).ok(),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identified_audiences_require_a_reference() {
        assert_eq!(Audience::principal(""), Err(AudienceError::Empty));
        assert_eq!(Audience::shared(""), Err(AudienceError::Empty));
        assert!(Audience::principal("person-a").is_ok());
        assert!(Audience::shared("room-a").is_ok());
    }
}
