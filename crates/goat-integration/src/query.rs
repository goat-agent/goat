use thiserror::Error;

use crate::IntegrationError;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Token {
    pub raw: String,
    pub negated: bool,
    pub kind: TokenKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TokenKind {
    Pair { key: String, value: TokenValue },
    Term(TokenValue),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TokenValue {
    Text(String),
    SelfRef,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Residue {
    Keep,
    Reject,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TermPolicy {
    Collect,
    Reject,
}

#[derive(Clone, Copy, Debug)]
pub struct LimitSpec {
    pub default: usize,
    pub max: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Arity {
    Single,
    Many,
}

#[derive(Clone, Copy, Debug)]
pub enum ValueSpec {
    Any,
    OneOf(&'static [&'static str]),
}

#[derive(Clone, Copy, Debug)]
pub struct KeySpec {
    pub key: &'static str,
    pub arity: Arity,
    pub negatable: bool,
    pub selfref: bool,
    pub values: ValueSpec,
}

impl KeySpec {
    #[must_use]
    pub const fn new(key: &'static str) -> Self {
        Self {
            key,
            arity: Arity::Single,
            negatable: false,
            selfref: false,
            values: ValueSpec::Any,
        }
    }

    #[must_use]
    pub const fn many(mut self) -> Self {
        self.arity = Arity::Many;
        self
    }

    #[must_use]
    pub const fn negatable(mut self) -> Self {
        self.negatable = true;
        self
    }

    #[must_use]
    pub const fn selfref(mut self) -> Self {
        self.selfref = true;
        self
    }

    #[must_use]
    pub const fn one_of(mut self, values: &'static [&'static str]) -> Self {
        self.values = ValueSpec::OneOf(values);
        self
    }
}

#[derive(Clone, Copy, Debug)]
pub struct WatchVocabulary {
    pub integration: &'static str,
    pub residue: Residue,
    pub terms: TermPolicy,
    pub limit: Option<LimitSpec>,
    pub keys: &'static [KeySpec],
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Match {
    pub key: &'static str,
    pub negated: bool,
    pub value: TokenValue,
}

#[derive(Debug, Default)]
pub struct ResolvedQuery {
    pub matched: Vec<Match>,
    pub residue: Vec<Token>,
    pub terms: Vec<String>,
    pub limit: Option<usize>,
}

impl ResolvedQuery {
    #[must_use]
    pub fn single(&self, key: &str) -> Option<&Match> {
        self.matched.iter().find(|m| m.key == key)
    }

    pub fn many<'a>(&'a self, key: &'a str) -> impl Iterator<Item = &'a Match> {
        self.matched.iter().filter(move |m| m.key == key)
    }

    #[must_use]
    pub fn state(&self, name: &str) -> Option<bool> {
        self.matched
            .iter()
            .find(|m| m.key == "is" && matches!(&m.value, TokenValue::Text(t) if t == name))
            .map(|m| !m.negated)
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
#[non_exhaustive]
pub enum QueryError {
    #[error("unbalanced quote at byte {0}")]
    UnbalancedQuote(usize),
    #[error("`{0}:` needs a value")]
    DanglingKey(String),
    #[error("{integration} does not understand `{key}:`; known keys: {known}")]
    UnknownKey {
        integration: &'static str,
        key: String,
        known: String,
    },
    #[error("{integration} does not take free text; found `{term}`")]
    FreeText {
        integration: &'static str,
        term: String,
    },
    #[error("`{0}:` may appear only once")]
    Repeated(String),
    #[error("`{key}:{value}` is not valid; allowed: {allowed}")]
    BadValue {
        key: String,
        value: String,
        allowed: String,
    },
    #[error("`{0}:` cannot be negated")]
    NotNegatable(String),
    #[error("`{0}:` does not take @me")]
    NoSelfRef(String),
    #[error("{0} does not support `limit:`")]
    LimitUnsupported(&'static str),
    #[error("`limit:{given}` is out of range; expected 1..={max}")]
    LimitRange { given: String, max: usize },
    #[error("{0}")]
    Invalid(String),
}

impl From<QueryError> for IntegrationError {
    fn from(error: QueryError) -> Self {
        IntegrationError::Config(error.to_string())
    }
}

pub fn parse(input: &str) -> Result<Vec<Token>, QueryError> {
    let bytes = input.as_bytes();
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i].is_ascii_whitespace() {
            i += 1;
            continue;
        }
        let start = i;
        let negated = bytes[i] == b'-';
        if negated {
            i += 1;
        }
        let (kind, end) = scan_token(input, i)?;
        tokens.push(Token {
            raw: input[start..end].to_owned(),
            negated,
            kind,
        });
        i = end;
    }
    Ok(tokens)
}

fn scan_token(input: &str, body: usize) -> Result<(TokenKind, usize), QueryError> {
    let bytes = input.as_bytes();
    if body < bytes.len() && bytes[body] == b'"' {
        let (text, end) = scan_quoted(input, body)?;
        return Ok((TokenKind::Term(TokenValue::Text(text)), end));
    }
    let mut i = body;
    let mut colon: Option<usize> = None;
    while i < bytes.len() && !bytes[i].is_ascii_whitespace() {
        match bytes[i] {
            b':' if colon.is_none() => {
                colon = Some(i);
                if is_key(&input[body..i]) && i + 1 < bytes.len() && bytes[i + 1] == b'"' {
                    let key = input[body..i].to_ascii_lowercase();
                    let (text, end) = scan_quoted(input, i + 1)?;
                    return Ok((
                        TokenKind::Pair {
                            key,
                            value: TokenValue::Text(text),
                        },
                        end,
                    ));
                }
                i += 1;
            }
            b'"' => return Err(QueryError::UnbalancedQuote(i)),
            _ => i += 1,
        }
    }
    let text = &input[body..i];
    match colon {
        Some(at) if is_key(&input[body..at]) => {
            let key = input[body..at].to_ascii_lowercase();
            let value = &input[at + 1..i];
            if value.is_empty() {
                return Err(QueryError::DanglingKey(key));
            }
            Ok((
                TokenKind::Pair {
                    key,
                    value: bare_value(value),
                },
                i,
            ))
        }
        _ => Ok((TokenKind::Term(bare_value(text)), i)),
    }
}

fn scan_quoted(input: &str, open: usize) -> Result<(String, usize), QueryError> {
    let bytes = input.as_bytes();
    let mut text = String::new();
    let mut i = open + 1;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' if i + 1 < bytes.len() && (bytes[i + 1] == b'\\' || bytes[i + 1] == b'"') => {
                text.push(bytes[i + 1] as char);
                i += 2;
            }
            b'"' => {
                let end = i + 1;
                if end < bytes.len() && !bytes[end].is_ascii_whitespace() {
                    return Err(QueryError::UnbalancedQuote(end));
                }
                return Ok((text, end));
            }
            _ => {
                let ch = input[i..].chars().next().unwrap_or('\u{fffd}');
                text.push(ch);
                i += ch.len_utf8();
            }
        }
    }
    Err(QueryError::UnbalancedQuote(open))
}

fn is_key(text: &str) -> bool {
    let mut chars = text.chars();
    chars
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.')
}

fn bare_value(text: &str) -> TokenValue {
    if text.eq_ignore_ascii_case("@me") {
        TokenValue::SelfRef
    } else {
        TokenValue::Text(text.to_owned())
    }
}

pub fn resolve(vocab: &WatchVocabulary, tokens: Vec<Token>) -> Result<ResolvedQuery, QueryError> {
    let mut resolved = ResolvedQuery::default();
    for token in tokens {
        match &token.kind {
            TokenKind::Pair { key, value } if key == "limit" => {
                let Some(spec) = vocab.limit else {
                    return Err(QueryError::LimitUnsupported(vocab.integration));
                };
                if token.negated {
                    return Err(QueryError::NotNegatable("limit".to_owned()));
                }
                if resolved.limit.is_some() {
                    return Err(QueryError::Repeated("limit".to_owned()));
                }
                let TokenValue::Text(text) = value else {
                    return Err(QueryError::LimitRange {
                        given: "@me".to_owned(),
                        max: spec.max,
                    });
                };
                let parsed: usize = text.parse().map_err(|_| QueryError::LimitRange {
                    given: text.clone(),
                    max: spec.max,
                })?;
                if parsed == 0 || parsed > spec.max {
                    return Err(QueryError::LimitRange {
                        given: text.clone(),
                        max: spec.max,
                    });
                }
                resolved.limit = Some(parsed);
            }
            TokenKind::Pair { key, value } => {
                let Some(spec) = vocab.keys.iter().find(|s| s.key == key) else {
                    match vocab.residue {
                        Residue::Keep => resolved.residue.push(token),
                        Residue::Reject => {
                            return Err(QueryError::UnknownKey {
                                integration: vocab.integration,
                                key: key.clone(),
                                known: known_keys(vocab),
                            });
                        }
                    }
                    continue;
                };
                if token.negated && !spec.negatable {
                    return Err(QueryError::NotNegatable(key.clone()));
                }
                if matches!(value, TokenValue::SelfRef) && !spec.selfref {
                    return Err(QueryError::NoSelfRef(key.clone()));
                }
                if spec.arity == Arity::Single && resolved.single(spec.key).is_some() {
                    return Err(QueryError::Repeated(key.clone()));
                }
                let value = match (&spec.values, value) {
                    (ValueSpec::OneOf(allowed), TokenValue::Text(text)) => {
                        let lowered = text.to_ascii_lowercase();
                        if !allowed.contains(&lowered.as_str()) {
                            return Err(QueryError::BadValue {
                                key: key.clone(),
                                value: text.clone(),
                                allowed: allowed.join(", "),
                            });
                        }
                        TokenValue::Text(lowered)
                    }
                    _ => value.clone(),
                };
                resolved.matched.push(Match {
                    key: spec.key,
                    negated: token.negated,
                    value,
                });
            }
            TokenKind::Term(value) => match vocab.residue {
                Residue::Keep => resolved.residue.push(token),
                Residue::Reject => match vocab.terms {
                    TermPolicy::Collect => resolved.terms.push(match value {
                        TokenValue::Text(text) => text.clone(),
                        TokenValue::SelfRef => "@me".to_owned(),
                    }),
                    TermPolicy::Reject => {
                        return Err(QueryError::FreeText {
                            integration: vocab.integration,
                            term: token.raw,
                        });
                    }
                },
            },
        }
    }
    if resolved.limit.is_none() {
        resolved.limit = vocab.limit.map(|spec| spec.default);
    }
    Ok(resolved)
}

fn known_keys(vocab: &WatchVocabulary) -> String {
    let mut keys: Vec<&str> = vocab.keys.iter().map(|s| s.key).collect();
    if vocab.limit.is_some() {
        keys.push("limit");
    }
    keys.sort_unstable();
    keys.join(", ")
}

#[derive(Clone, Copy, Debug)]
pub enum SelfRefStyle<'a> {
    Native,
    Replace(&'a str),
}

#[must_use]
pub fn render(tokens: &[Token], style: SelfRefStyle<'_>) -> String {
    let mut parts = Vec::with_capacity(tokens.len());
    for token in tokens {
        let rebuilt = match (&style, &token.kind) {
            (SelfRefStyle::Replace(with), TokenKind::Pair { key, value })
                if *value == TokenValue::SelfRef =>
            {
                Some(format!(
                    "{}{key}:{with}",
                    if token.negated { "-" } else { "" }
                ))
            }
            (SelfRefStyle::Replace(with), TokenKind::Term(TokenValue::SelfRef)) => {
                Some(format!("{}{with}", if token.negated { "-" } else { "" }))
            }
            _ => None,
        };
        parts.push(rebuilt.unwrap_or_else(|| token.raw.clone()));
    }
    parts.join(" ")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Comparison {
    Eq,
    Gt,
    Gte,
    Lt,
    Lte,
}

#[must_use]
pub fn split_comparison(value: &str) -> (Comparison, &str) {
    if let Some(rest) = value.strip_prefix(">=") {
        (Comparison::Gte, rest)
    } else if let Some(rest) = value.strip_prefix("<=") {
        (Comparison::Lte, rest)
    } else if let Some(rest) = value.strip_prefix('>') {
        (Comparison::Gt, rest)
    } else if let Some(rest) = value.strip_prefix('<') {
        (Comparison::Lt, rest)
    } else {
        (Comparison::Eq, value)
    }
}

pub fn assert_vocabulary(vocab: &WatchVocabulary) {
    let mut seen: Vec<&str> = Vec::new();
    for spec in vocab.keys {
        assert!(
            spec.key == spec.key.to_ascii_lowercase(),
            "{}: key `{}` must be lowercase",
            vocab.integration,
            spec.key
        );
        assert!(
            spec.key != "limit",
            "{}: `limit` is resolver-reserved; declare LimitSpec instead",
            vocab.integration
        );
        assert!(
            !seen.contains(&spec.key),
            "{}: key `{}` is declared twice",
            vocab.integration,
            spec.key
        );
        seen.push(spec.key);
        if let ValueSpec::OneOf(allowed) = spec.values {
            assert!(
                !allowed.is_empty(),
                "{}: `{}` has an empty OneOf",
                vocab.integration,
                spec.key
            );
            assert!(
                !spec.selfref,
                "{}: `{}` cannot be both OneOf and selfref",
                vocab.integration, spec.key
            );
            for value in allowed {
                assert!(
                    *value == value.to_ascii_lowercase(),
                    "{}: `{}` OneOf value `{}` must be lowercase",
                    vocab.integration,
                    spec.key,
                    value
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VOCAB: WatchVocabulary = WatchVocabulary {
        integration: "acme",
        residue: Residue::Reject,
        terms: TermPolicy::Collect,
        limit: Some(LimitSpec {
            default: 50,
            max: 250,
        }),
        keys: &[
            KeySpec::new("assignee").selfref(),
            KeySpec::new("team"),
            KeySpec::new("label").many(),
            KeySpec::new("is")
                .many()
                .negatable()
                .one_of(&["open", "closed"]),
        ],
    };

    const PASSTHROUGH: WatchVocabulary = WatchVocabulary {
        integration: "hub",
        residue: Residue::Keep,
        terms: TermPolicy::Reject,
        limit: Some(LimitSpec {
            default: 50,
            max: 100,
        }),
        keys: &[],
    };

    fn pair(token: &Token) -> (&str, &TokenValue) {
        match &token.kind {
            TokenKind::Pair { key, value } => (key, value),
            TokenKind::Term(_) => panic!("expected a pair"),
        }
    }

    #[test]
    fn pairs_terms_and_negation_tokenize() {
        let tokens = parse("assignee:@me -is:closed fix login").unwrap();
        assert_eq!(tokens.len(), 4);
        assert_eq!(pair(&tokens[0]), ("assignee", &TokenValue::SelfRef));
        assert!(tokens[1].negated);
        assert_eq!(
            pair(&tokens[1]),
            ("is", &TokenValue::Text("closed".to_owned()))
        );
        assert_eq!(
            tokens[2].kind,
            TokenKind::Term(TokenValue::Text("fix".to_owned()))
        );
        assert_eq!(tokens[1].raw, "-is:closed");
    }

    #[test]
    fn keys_fold_to_lowercase_but_values_do_not() {
        let tokens = parse("Team:ENG").unwrap();
        assert_eq!(
            pair(&tokens[0]),
            ("team", &TokenValue::Text("ENG".to_owned()))
        );
        assert_eq!(tokens[0].raw, "Team:ENG");
    }

    #[test]
    fn only_an_unquoted_at_me_is_a_selfref() {
        let tokens = parse("assignee:@ME \"@me\" @me x:\"@me\"").unwrap();
        assert_eq!(pair(&tokens[0]), ("assignee", &TokenValue::SelfRef));
        assert_eq!(
            tokens[1].kind,
            TokenKind::Term(TokenValue::Text("@me".to_owned()))
        );
        assert_eq!(tokens[2].kind, TokenKind::Term(TokenValue::SelfRef));
        assert_eq!(pair(&tokens[3]), ("x", &TokenValue::Text("@me".to_owned())));
    }

    #[test]
    fn quoted_values_keep_spaces_and_escapes() {
        let tokens = parse(r#"label:"in \"review\"" "free \\ term""#).unwrap();
        assert_eq!(
            pair(&tokens[0]),
            ("label", &TokenValue::Text(r#"in "review""#.to_owned()))
        );
        assert_eq!(tokens[0].raw, r#"label:"in \"review\"""#);
        assert_eq!(
            tokens[1].kind,
            TokenKind::Term(TokenValue::Text(r"free \ term".to_owned()))
        );
    }

    #[test]
    fn a_value_may_contain_colons() {
        let tokens = parse("view:https://notion.so/x?v=1").unwrap();
        assert_eq!(
            pair(&tokens[0]),
            (
                "view",
                &TokenValue::Text("https://notion.so/x?v=1".to_owned())
            )
        );
    }

    #[test]
    fn invalid_key_prefixes_stay_terms() {
        let tokens = parse(":foo 3d:x --x").unwrap();
        assert_eq!(
            tokens[0].kind,
            TokenKind::Term(TokenValue::Text(":foo".to_owned()))
        );
        assert_eq!(
            tokens[1].kind,
            TokenKind::Term(TokenValue::Text("3d:x".to_owned()))
        );
        assert!(tokens[2].negated);
        assert_eq!(
            tokens[2].kind,
            TokenKind::Term(TokenValue::Text("-x".to_owned()))
        );
    }

    #[test]
    fn broken_quotes_error_with_a_position() {
        assert!(matches!(
            parse(r#"label:"open"#),
            Err(QueryError::UnbalancedQuote(_))
        ));
        assert!(matches!(
            parse(r#"la"bel:x"#),
            Err(QueryError::UnbalancedQuote(2))
        ));
        assert!(matches!(
            parse(r#""closed"x"#),
            Err(QueryError::UnbalancedQuote(8))
        ));
        assert!(matches!(
            parse(r#"key:va"lue"#),
            Err(QueryError::UnbalancedQuote(6))
        ));
    }

    #[test]
    fn a_dangling_key_errors_but_a_quoted_empty_value_parses() {
        assert_eq!(
            parse("label:"),
            Err(QueryError::DanglingKey("label".to_owned()))
        );
        let tokens = parse(r#"label:"""#).unwrap();
        assert_eq!(
            pair(&tokens[0]),
            ("label", &TokenValue::Text(String::new()))
        );
    }

    #[test]
    fn resolve_fills_the_default_limit_and_parses_an_explicit_one() {
        let resolved = resolve(&VOCAB, parse("assignee:@me").unwrap()).unwrap();
        assert_eq!(resolved.limit, Some(50));
        let resolved = resolve(&VOCAB, parse("limit:25").unwrap()).unwrap();
        assert_eq!(resolved.limit, Some(25));
    }

    #[test]
    fn limit_violations_are_loud() {
        assert!(matches!(
            resolve(&VOCAB, parse("limit:0").unwrap()),
            Err(QueryError::LimitRange { .. })
        ));
        assert!(matches!(
            resolve(&VOCAB, parse("limit:9999").unwrap()),
            Err(QueryError::LimitRange { .. })
        ));
        assert!(matches!(
            resolve(&VOCAB, parse("limit:abc").unwrap()),
            Err(QueryError::LimitRange { .. })
        ));
        assert!(matches!(
            resolve(&VOCAB, parse("limit:5 limit:6").unwrap()),
            Err(QueryError::Repeated(_))
        ));
        assert!(matches!(
            resolve(&VOCAB, parse("-limit:5").unwrap()),
            Err(QueryError::NotNegatable(_))
        ));
        let unsupported = WatchVocabulary {
            limit: None,
            ..VOCAB
        };
        assert!(matches!(
            resolve(&unsupported, parse("limit:5").unwrap()),
            Err(QueryError::LimitUnsupported("acme"))
        ));
    }

    #[test]
    fn an_unknown_key_names_the_known_ones() {
        let err = resolve(&VOCAB, parse("asignee:@me").unwrap()).unwrap_err();
        let QueryError::UnknownKey { key, known, .. } = err else {
            panic!("expected UnknownKey");
        };
        assert_eq!(key, "asignee");
        assert_eq!(known, "assignee, is, label, limit, team");
    }

    #[test]
    fn keep_residue_passes_unknown_tokens_through() {
        let resolved = resolve(
            &PASSTHROUGH,
            parse("is:open review-requested:@me weird:x").unwrap(),
        )
        .unwrap();
        assert!(resolved.matched.is_empty());
        assert_eq!(resolved.residue.len(), 3);
    }

    #[test]
    fn free_text_collects_or_rejects_per_policy() {
        let resolved = resolve(&VOCAB, parse("team:ENG 결제 오류").unwrap()).unwrap();
        assert_eq!(resolved.terms, vec!["결제", "오류"]);
        let rejecting = WatchVocabulary {
            terms: TermPolicy::Reject,
            ..VOCAB
        };
        assert!(matches!(
            resolve(&rejecting, parse("team:ENG stray").unwrap()),
            Err(QueryError::FreeText { .. })
        ));
    }

    #[test]
    fn single_keys_reject_repeats_and_many_keys_accumulate() {
        assert!(matches!(
            resolve(&VOCAB, parse("team:a team:b").unwrap()),
            Err(QueryError::Repeated(_))
        ));
        let resolved = resolve(&VOCAB, parse("label:a label:b").unwrap()).unwrap();
        assert_eq!(resolved.many("label").count(), 2);
    }

    #[test]
    fn one_of_values_fold_and_reject() {
        let resolved = resolve(&VOCAB, parse("is:OPEN").unwrap()).unwrap();
        assert_eq!(resolved.state("open"), Some(true));
        let err = resolve(&VOCAB, parse("is:done").unwrap()).unwrap_err();
        assert!(matches!(err, QueryError::BadValue { .. }));
    }

    #[test]
    fn negation_and_selfref_are_gated_per_key() {
        assert!(matches!(
            resolve(&VOCAB, parse("-team:ENG").unwrap()),
            Err(QueryError::NotNegatable(_))
        ));
        assert!(matches!(
            resolve(&VOCAB, parse("team:@me").unwrap()),
            Err(QueryError::NoSelfRef(_))
        ));
        let resolved = resolve(&VOCAB, parse("-is:closed").unwrap()).unwrap();
        assert_eq!(resolved.state("closed"), Some(false));
    }

    #[test]
    fn render_native_keeps_raw_bytes() {
        let query = r#"is:open  review-requested:@me label:"in review""#;
        let tokens = parse(query).unwrap();
        assert_eq!(
            render(&tokens, SelfRefStyle::Native),
            r#"is:open review-requested:@me label:"in review""#
        );
    }

    #[test]
    fn render_replace_rewrites_only_selfrefs() {
        let tokens = parse("assigned:@me -author:@me @me is:open").unwrap();
        assert_eq!(
            render(&tokens, SelfRefStyle::Replace("<@U1>")),
            "assigned:<@U1> -author:<@U1> <@U1> is:open"
        );
    }

    #[test]
    fn comparisons_split_off_values() {
        assert_eq!(split_comparison(">=5"), (Comparison::Gte, "5"));
        assert_eq!(split_comparison("<=5"), (Comparison::Lte, "5"));
        assert_eq!(split_comparison(">2024"), (Comparison::Gt, "2024"));
        assert_eq!(split_comparison("<x"), (Comparison::Lt, "x"));
        assert_eq!(split_comparison("plain"), (Comparison::Eq, "plain"));
    }

    #[test]
    fn the_vocabulary_invariants_hold() {
        assert_vocabulary(&VOCAB);
        assert_vocabulary(&PASSTHROUGH);
    }

    #[test]
    #[should_panic(expected = "declared twice")]
    fn duplicate_keys_fail_the_invariant() {
        const KEYS: &[KeySpec] = &[KeySpec::new("a"), KeySpec::new("a")];
        assert_vocabulary(&WatchVocabulary {
            keys: KEYS,
            ..VOCAB
        });
    }

    #[test]
    #[should_panic(expected = "resolver-reserved")]
    fn a_limit_key_fails_the_invariant() {
        const KEYS: &[KeySpec] = &[KeySpec::new("limit")];
        assert_vocabulary(&WatchVocabulary {
            keys: KEYS,
            ..VOCAB
        });
    }

    #[test]
    #[should_panic(expected = "OneOf and selfref")]
    fn one_of_selfref_fails_the_invariant() {
        const KEYS: &[KeySpec] = &[KeySpec::new("state").selfref().one_of(&["open"])];
        assert_vocabulary(&WatchVocabulary {
            keys: KEYS,
            ..VOCAB
        });
    }

    #[test]
    fn empty_input_parses_to_no_tokens() {
        assert_eq!(parse("").unwrap(), Vec::new());
        assert_eq!(parse("   ").unwrap(), Vec::new());
    }
}
