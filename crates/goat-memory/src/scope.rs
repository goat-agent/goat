use std::fmt;
use std::str::FromStr;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct DomainName(String);

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Scope {
    Owner,
    Self_,
    Domain(DomainName),
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ScopeError {
    #[error("empty scope")]
    Empty,
    #[error("invalid domain name: {0:?}")]
    InvalidDomain(String),
    #[error("unknown scope: {0:?}")]
    Unknown(String),
}

impl Scope {
    pub fn domain(name: impl Into<String>) -> Result<Self, ScopeError> {
        let name = name.into();
        if valid_domain(&name) {
            Ok(Scope::Domain(DomainName(name)))
        } else {
            Err(ScopeError::InvalidDomain(name))
        }
    }

    pub fn as_key(&self) -> String {
        match self {
            Scope::Owner => "owner".to_string(),
            Scope::Self_ => "self".to_string(),
            Scope::Domain(name) => format!("domain:{}", name.0),
        }
    }

    pub fn as_path_segment(&self) -> String {
        match self {
            Scope::Owner => "owner".to_string(),
            Scope::Self_ => "self".to_string(),
            Scope::Domain(name) => format!("domain/{}", name.0),
        }
    }
}

fn valid_domain(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

impl fmt::Display for Scope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.as_key())
    }
}

impl FromStr for Scope {
    type Err = ScopeError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.is_empty() {
            return Err(ScopeError::Empty);
        }
        match s {
            "owner" => Ok(Scope::Owner),
            "self" => Ok(Scope::Self_),
            other => {
                if let Some(name) = other.strip_prefix("domain:") {
                    Scope::domain(name)
                } else {
                    Err(ScopeError::Unknown(other.to_string()))
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_roundtrip() {
        for s in [
            Scope::Owner,
            Scope::Self_,
            Scope::domain("dev").unwrap(),
            Scope::domain("home").unwrap(),
        ] {
            let key = s.as_key();
            assert_eq!(Scope::from_str(&key).unwrap(), s);
        }
    }

    #[test]
    fn path_segments() {
        assert_eq!(Scope::Owner.as_path_segment(), "owner");
        assert_eq!(Scope::Self_.as_path_segment(), "self");
        assert_eq!(
            Scope::domain("dev").unwrap().as_path_segment(),
            "domain/dev"
        );
    }

    #[test]
    fn rejects_bad_domains() {
        assert!(Scope::domain("").is_err());
        assert!(Scope::domain("Dev").is_err());
        assert!(Scope::domain("a/b").is_err());
        assert!(Scope::domain("a b").is_err());
        assert!(Scope::domain("ok-1_2").is_ok());
    }

    #[test]
    fn parse_errors() {
        assert_eq!(Scope::from_str(""), Err(ScopeError::Empty));
        assert!(matches!(
            Scope::from_str("bogus"),
            Err(ScopeError::Unknown(_))
        ));
        assert!(matches!(
            Scope::from_str("domain:Bad"),
            Err(ScopeError::InvalidDomain(_))
        ));
    }
}
