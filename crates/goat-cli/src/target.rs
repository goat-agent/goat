use goat_core::account;
use goat_core::catalog::Model;

pub(crate) const DEFAULT_EFFORT: &str = "default";

pub(crate) struct Offer {
    pub(crate) account: account::Id,
    pub(crate) kind: account::Kind,
    pub(crate) label: String,
}

pub(crate) struct Current<'a> {
    pub(crate) account: account::Id,
    pub(crate) model: &'a str,
    pub(crate) effort: Option<&'a str>,
}

pub(crate) struct Choices<'a> {
    pub(crate) offers: &'a [Offer],
    pub(crate) catalog: &'a [Model],
    pub(crate) current: Current<'a>,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Candidate {
    pub(crate) value: String,
    pub(crate) label: String,
    pub(crate) is_current: bool,
    pub(crate) efforts: Vec<String>,
    pub(crate) effort: usize,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Selection {
    pub(crate) account: account::Id,
    pub(crate) model: String,
    pub(crate) effort: Effort,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Effort {
    Keep,
    Default,
    Level(String),
}

impl Choices<'_> {
    pub(crate) fn models(&self, typed: &str) -> Vec<Candidate> {
        let mut found = Vec::new();
        match typed.split_once('/') {
            None => {
                for offer in self.offers {
                    let value = format!("{}/", offer.label);
                    if matches(&value, typed) {
                        found.push(Candidate {
                            label: value.clone(),
                            value,
                            is_current: offer.account == self.current.account,
                            efforts: Vec::new(),
                            effort: 0,
                        });
                    }
                }
                for model in self.catalog {
                    let is_offered = self.offers.iter().any(|offer| offer.kind == model.kind);
                    let is_new = found.iter().all(|seen| seen.value != *model.id);
                    if is_offered && is_new && matches(&model.id, typed) {
                        found.push(self.model_candidate(model.id.to_string(), model));
                    }
                }
            }
            Some((label, rest)) => {
                let Some(offer) = self.offer(label) else {
                    return found;
                };
                for model in self.models_of(offer.kind) {
                    if matches(&model.id, rest) {
                        let mut candidate =
                            self.model_candidate(format!("{label}/{}", model.id), model);
                        candidate.is_current &= offer.account == self.current.account;
                        found.push(candidate);
                    }
                }
            }
        }
        let wanted = typed.rsplit('/').next().unwrap_or(typed);
        found.sort_by_key(|candidate| candidate.label != wanted);
        found
    }

    pub(crate) fn efforts(&self, typed: &str) -> Vec<Candidate> {
        let Some(model) = self.current_model() else {
            return Vec::new();
        };
        let current = self.current.effort.unwrap_or(DEFAULT_EFFORT);
        levels_of(model)
            .into_iter()
            .filter(|level| matches(level, typed))
            .map(|level| Candidate {
                is_current: level == current,
                label: level.clone(),
                value: level,
                efforts: Vec::new(),
                effort: 0,
            })
            .collect()
    }

    pub(crate) fn resolve(&self, spec: &str) -> Result<Selection, String> {
        let (named, rest) = match spec.split_once('/') {
            Some((label, rest)) => {
                let offer = self
                    .offer(label)
                    .ok_or_else(|| format!("no account named {label}"))?;
                (Some(offer), rest)
            }
            None => (None, spec),
        };
        let (id, level) = self.split_effort(named, rest);
        let offer = match named {
            Some(offer) => {
                if self.model_of(offer, id).is_none() {
                    return Err(format!("{} does not offer {id}", offer.label));
                }
                offer
            }
            None => self.offering(id)?,
        };
        let effort = match level {
            None => Effort::Keep,
            Some(DEFAULT_EFFORT) => Effort::Default,
            Some(level) => {
                if let Some(model) = self.model_of(offer, id)
                    && !model.accepts(level)
                {
                    return Err(format!("{} offers effort {}", model.name(), levels(model)));
                }
                Effort::Level(level.to_owned())
            }
        };
        Ok(Selection {
            account: offer.account,
            model: id.to_owned(),
            effort,
        })
    }

    pub(crate) fn effort(&self, level: &str) -> Result<Effort, String> {
        if level == DEFAULT_EFFORT {
            return Ok(Effort::Default);
        }
        if let Some(model) = self.current_model()
            && !model.accepts(level)
        {
            return Err(format!("{} offers effort {}", model.name(), levels(model)));
        }
        Ok(Effort::Level(level.to_owned()))
    }

    pub(crate) fn name(&self, account: account::Id, id: &str) -> String {
        self.offers
            .iter()
            .find(|offer| offer.account == account)
            .and_then(|offer| self.model_of(offer, id))
            .map_or_else(|| id.to_owned(), |model| model.name().to_owned())
    }

    pub(crate) fn current_model(&self) -> Option<&Model> {
        self.model_of(self.current_offer()?, self.current.model)
    }

    fn models_of(&self, kind: account::Kind) -> impl Iterator<Item = &Model> {
        self.catalog.iter().filter(move |model| model.kind == kind)
    }

    fn model_of(&self, offer: &Offer, id: &str) -> Option<&Model> {
        self.models_of(offer.kind).find(|model| &*model.id == id)
    }

    fn offer(&self, label: &str) -> Option<&Offer> {
        self.offers.iter().find(|offer| offer.label == label)
    }

    fn current_offer(&self) -> Option<&Offer> {
        self.offers
            .iter()
            .find(|offer| offer.account == self.current.account)
    }

    fn is_known(&self, named: Option<&Offer>, id: &str) -> bool {
        match named {
            Some(offer) => self.model_of(offer, id).is_some(),
            None => self
                .offers
                .iter()
                .any(|offer| self.model_of(offer, id).is_some()),
        }
    }

    fn split_effort<'s>(&self, named: Option<&Offer>, rest: &'s str) -> (&'s str, Option<&'s str>) {
        if self.is_known(named, rest) {
            return (rest, None);
        }
        match rest.rsplit_once(':') {
            Some((id, level)) if self.is_known(named, id) => (id, Some(level)),
            _ => (rest, None),
        }
    }

    fn offering(&self, id: &str) -> Result<&Offer, String> {
        if let Some(offer) = self.current_offer()
            && self.model_of(offer, id).is_some()
        {
            return Ok(offer);
        }
        let offering: Vec<&Offer> = self
            .offers
            .iter()
            .filter(|offer| self.model_of(offer, id).is_some())
            .collect();
        match offering.as_slice() {
            [offer] => Ok(offer),
            [] => Err(format!("no account offers {id}")),
            several => {
                let labels: Vec<&str> = several.iter().map(|offer| offer.label.as_str()).collect();
                Err(format!(
                    "{id} is on {}, so name the account",
                    joined(&labels)
                ))
            }
        }
    }

    fn model_candidate(&self, value: String, model: &Model) -> Candidate {
        let efforts = levels_of(model);
        let current = self.current.effort.unwrap_or(DEFAULT_EFFORT);
        Candidate {
            value,
            label: model.id.to_string(),
            is_current: *model.id == *self.current.model,
            effort: efforts
                .iter()
                .position(|level| level == current)
                .unwrap_or(0),
            efforts,
        }
    }
}

pub(crate) fn levels(model: &Model) -> String {
    match model.efforts.as_deref() {
        Some([]) => "none".to_owned(),
        Some(efforts) => efforts.join(", "),
        None => "unknown".to_owned(),
    }
}

fn matches(text: &str, typed: &str) -> bool {
    let text = text.to_lowercase();
    let typed = typed.to_lowercase();
    text.starts_with(&typed)
        || text
            .match_indices(['-', '.', ':', '/'])
            .any(|(at, separator)| text[at + separator.len()..].starts_with(&typed))
}

fn levels_of(model: &Model) -> Vec<String> {
    match model.efforts.as_deref() {
        Some(levels) if !levels.is_empty() => std::iter::once(DEFAULT_EFFORT)
            .chain(levels.iter().map(|level| &**level))
            .map(str::to_owned)
            .collect(),
        _ => Vec::new(),
    }
}

fn joined(labels: &[&str]) -> String {
    match labels {
        [] => String::new(),
        [only] => (*only).to_owned(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

#[cfg(test)]
mod tests {
    use goat_core::account::{Account, Credential, Kind};

    use super::*;

    fn model(kind: Kind, id: &str, name: Option<&str>, efforts: &[&str]) -> Model {
        Model {
            id: id.into(),
            kind,
            display_name: name.map(Into::into),
            efforts: Some(efforts.iter().map(|level| (*level).into()).collect()),
            default_effort: None,
        }
    }

    fn catalog() -> Vec<Model> {
        vec![
            model(
                Kind::OpenAi,
                "gpt-6.2-sol",
                Some("GPT-6.2 Sol"),
                &["low", "high"],
            ),
            model(Kind::OpenAi, "gpt-6.2-astra", None, &["high"]),
            model(
                Kind::Anthropic,
                "claude-opus-5-5",
                Some("Claude Opus 5.5"),
                &["high", "max"],
            ),
        ]
    }

    fn offer(kind: Kind, name: &str) -> Offer {
        let account = Account::new(kind, name, Credential::ApiKey { key: "k".into() });
        Offer {
            account: account.id,
            kind,
            label: account.label(),
        }
    }

    fn offers() -> Vec<Offer> {
        vec![
            offer(Kind::OpenAi, "codex"),
            offer(Kind::OpenAi, "work"),
            offer(Kind::Anthropic, "api"),
        ]
    }

    fn choices<'a>(offers: &'a [Offer], catalog: &'a [Model]) -> Choices<'a> {
        Choices {
            offers,
            catalog,
            current: Current {
                account: offers[0].account,
                model: "gpt-6.2-sol",
                effort: Some("high"),
            },
        }
    }

    fn labels(offers: &[Offer], typed: &str) -> Vec<String> {
        choices(offers, &catalog())
            .models(typed)
            .into_iter()
            .map(|candidate| candidate.label)
            .collect()
    }

    #[test]
    fn accounts_come_before_each_offered_model_once() {
        assert_eq!(
            labels(&offers(), ""),
            [
                "openai:codex/",
                "openai:work/",
                "anthropic:api/",
                "gpt-6.2-sol",
                "gpt-6.2-astra",
                "claude-opus-5-5"
            ]
        );
    }

    #[test]
    fn models_of_a_provider_without_an_account_are_left_out() {
        let offers = vec![offer(Kind::OpenAi, "codex")];
        assert_eq!(
            labels(&offers, ""),
            ["openai:codex/", "gpt-6.2-sol", "gpt-6.2-astra"]
        );
    }

    #[test]
    fn a_part_of_an_id_finds_it() {
        assert_eq!(labels(&offers(), "sol"), ["gpt-6.2-sol"]);
        assert_eq!(labels(&offers(), "OPUS"), ["claude-opus-5-5"]);
        assert_eq!(labels(&offers(), "codex"), ["openai:codex/"]);
        assert_eq!(labels(&offers(), "openai:work/astra"), ["gpt-6.2-astra"]);
    }

    #[test]
    fn an_exact_id_comes_first() {
        let offers = vec![offer(Kind::OpenAi, "codex")];
        let catalog = vec![
            model(Kind::OpenAi, "gpt-6.2-sol-mini", None, &[]),
            model(Kind::OpenAi, "gpt-6.2-sol", None, &[]),
        ];
        let labels: Vec<String> = choices(&offers, &catalog)
            .models("gpt-6.2-sol")
            .into_iter()
            .map(|candidate| candidate.label)
            .collect();
        assert_eq!(labels, ["gpt-6.2-sol", "gpt-6.2-sol-mini"]);
    }

    #[test]
    fn a_model_starts_at_the_current_effort_when_it_has_it() {
        let (offers, catalog) = (offers(), catalog());
        let choices = choices(&offers, &catalog);
        let sol = choices.models("sol");
        assert_eq!(sol[0].efforts, ["default", "low", "high"]);
        assert_eq!(sol[0].effort, 2);
        assert_eq!(choices.models("opus")[0].effort, 1);
    }

    #[test]
    fn efforts_list_the_levels_of_the_current_model() {
        let (offers, catalog) = (offers(), catalog());
        let found: Vec<(String, bool)> = choices(&offers, &catalog)
            .efforts("")
            .into_iter()
            .map(|candidate| (candidate.value, candidate.is_current))
            .collect();
        assert_eq!(
            found,
            [
                ("default".to_owned(), false),
                ("low".to_owned(), false),
                ("high".to_owned(), true)
            ]
        );
    }

    #[test]
    fn an_effort_the_model_lacks_is_an_error() {
        let (offers, catalog) = (offers(), catalog());
        let choices = choices(&offers, &catalog);
        assert_eq!(
            choices.effort("max"),
            Err("GPT-6.2 Sol offers effort low, high".to_owned())
        );
        assert_eq!(choices.effort("default"), Ok(Effort::Default));
    }

    #[test]
    fn a_model_on_the_current_account_keeps_the_account() {
        let (offers, catalog) = (offers(), catalog());
        let selection = choices(&offers, &catalog).resolve("gpt-6.2-sol").unwrap();
        assert_eq!(selection.account, offers[0].account);
        assert_eq!(selection.effort, Effort::Keep);
    }

    #[test]
    fn a_model_on_one_other_account_switches_to_it() {
        let (offers, catalog) = (offers(), catalog());
        let selection = choices(&offers, &catalog)
            .resolve("claude-opus-5-5:max")
            .unwrap();
        assert_eq!(
            selection,
            Selection {
                account: offers[2].account,
                model: "claude-opus-5-5".to_owned(),
                effort: Effort::Level("max".to_owned()),
            }
        );
    }

    #[test]
    fn a_model_on_several_other_accounts_needs_the_account() {
        let offers = vec![
            offer(Kind::Anthropic, "api"),
            offer(Kind::OpenAi, "codex"),
            offer(Kind::OpenAi, "work"),
        ];
        let catalog = catalog();
        let choices = Choices {
            offers: &offers,
            catalog: &catalog,
            current: Current {
                account: offers[0].account,
                model: "claude-opus-5-5",
                effort: None,
            },
        };
        assert_eq!(
            choices.resolve("gpt-6.2-sol"),
            Err("gpt-6.2-sol is on openai:codex and openai:work, so name the account".to_owned())
        );
    }

    #[test]
    fn a_named_account_is_used_as_given() {
        let (offers, catalog) = (offers(), catalog());
        let selection = choices(&offers, &catalog)
            .resolve("openai:work/gpt-6.2-sol:low")
            .unwrap();
        assert_eq!(selection.account, offers[1].account);
        assert_eq!(selection.effort, Effort::Level("low".to_owned()));
    }

    #[test]
    fn unknown_models_and_efforts_are_errors() {
        let (offers, catalog) = (offers(), catalog());
        let choices = choices(&offers, &catalog);
        assert_eq!(
            choices.resolve("gpt-6.3"),
            Err("no account offers gpt-6.3".to_owned())
        );
        assert_eq!(
            choices.resolve("anthropic:api/gpt-6.2-astra"),
            Err("anthropic:api does not offer gpt-6.2-astra".to_owned())
        );
        assert_eq!(
            choices.resolve("gpt-6.2-astra:low"),
            Err("gpt-6.2-astra offers effort high".to_owned())
        );
    }

    #[test]
    fn a_name_falls_back_to_the_id() {
        let (offers, catalog) = (offers(), catalog());
        let choices = choices(&offers, &catalog);
        assert_eq!(
            choices.name(offers[0].account, "gpt-6.2-sol"),
            "GPT-6.2 Sol"
        );
        assert_eq!(choices.name(offers[0].account, "gpt-6.3"), "gpt-6.3");
    }
}
