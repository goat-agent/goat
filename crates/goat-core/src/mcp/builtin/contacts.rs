use reqwest::Url;
use serde_json::{Value, json};
use tokio::sync::OnceCell;

use super::args::Args;
use super::google::{self, Rest, items};

pub(super) const SCOPES: &[&str] = &[
    "https://www.googleapis.com/auth/contacts.readonly",
    "https://www.googleapis.com/auth/directory.readonly",
    "https://www.googleapis.com/auth/userinfo.profile",
    "https://www.googleapis.com/auth/userinfo.email",
];
pub(super) const TOOLS: &str = include_str!("contacts.json");
const MASK: &str = "names,emailAddresses";
const PAGE: u64 = 10;
const MOST_CONTACTS: u64 = 30;
const MOST_PEOPLE: u64 = 500;

#[derive(Debug)]
pub(super) struct Contacts {
    rest: Rest,
    base: Box<str>,
    warm: OnceCell<()>,
}

impl Contacts {
    pub(super) fn new(rest: Rest, base: &str) -> Self {
        Self {
            rest,
            base: base.into(),
            warm: OnceCell::new(),
        }
    }

    pub(super) async fn run(&self, name: &str, args: &Args) -> Result<Value, String> {
        match name {
            "search_contacts" => self.search_contacts(args).await,
            "search_directory_people" => self.search_directory_people(args).await,
            "get_user_profile" => self.get_user_profile().await,
            _ => Err(google::no_tool(name)),
        }
    }

    async fn search_contacts(&self, args: &Args) -> Result<Value, String> {
        let search = |query: &str, size: u64| {
            self.url(
                "people:searchContacts",
                &[
                    ("query", query.to_owned()),
                    ("readMask", MASK.to_owned()),
                    ("pageSize", size.to_string()),
                ],
            )
        };
        self.warm
            .get_or_try_init(|| async { self.rest.get(search("", 1)?).await.map(drop) })
            .await?;
        let size = args.page("maxResults", PAGE, MOST_CONTACTS);
        let found = self
            .rest
            .get(search(args.word("query").unwrap_or_default(), size)?)
            .await?;
        Ok(json!({
            "results": items(&found, "results").map(|result| person(&result["person"])).collect::<Vec<_>>(),
        }))
    }

    async fn search_directory_people(&self, args: &Args) -> Result<Value, String> {
        let mut query = vec![
            ("query", args.word("query").unwrap_or_default().to_owned()),
            ("readMask", MASK.to_owned()),
            (
                "pageSize",
                args.page("pageSize", PAGE, MOST_PEOPLE).to_string(),
            ),
        ];
        let sources = args.list("sources");
        let sources = if sources.is_empty() {
            vec!["DOMAIN_PROFILE".to_owned()]
        } else {
            sources
        };
        query.extend(
            sources
                .into_iter()
                .map(|source| ("sources", format!("DIRECTORY_SOURCE_TYPE_{source}"))),
        );
        if let Some(token) = args.word("pageToken") {
            query.push(("pageToken", token.to_owned()));
        }
        let found = self
            .rest
            .get(self.url("people:searchDirectoryPeople", &query)?)
            .await?;
        let mut out = json!({
            "results": items(&found, "people").map(person).collect::<Vec<_>>(),
        });
        google::copy(&found, &mut out, "nextPageToken");
        Ok(out)
    }

    async fn get_user_profile(&self) -> Result<Value, String> {
        let found = self
            .rest
            .get(self.url("people/me", &[("personFields", MASK.to_owned())])?)
            .await?;
        let me = person(&found);
        Ok(json!({ "name": me["name"], "emailAddress": me["email"] }))
    }

    fn url(&self, path: &str, query: &[(&str, String)]) -> Result<Url, String> {
        let segments: Vec<&str> = std::iter::once("v1").chain(path.split('/')).collect();
        google::at(&self.base, &segments, query)
    }
}

fn person(found: &Value) -> Value {
    let primary = |key: &str| {
        let listed = &found[key];
        items(found, key)
            .find(|item| item["metadata"]["primary"].as_bool() == Some(true))
            .unwrap_or(&listed[0])
            .clone()
    };
    json!({
        "name": primary("names")["displayName"],
        "email": primary("emailAddresses")["value"],
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::super::google::Product;
    use super::super::google::fake::{self, serve};

    #[tokio::test]
    async fn contacts_are_searched_after_one_warmup() {
        let google = serve(vec![(
            "GET /v1/people:searchContacts",
            fake::json(&json!({ "results": [{ "person": {
                "names": [{ "displayName": "김철수" }],
                "emailAddresses": [
                    { "value": "old@example.com" },
                    { "value": "kim@example.com", "metadata": { "primary": true } },
                ],
            }}]})),
        )])
        .await;
        let server = google.server(Product::Contacts);

        let found = server
            .run("search_contacts", &fake::args(json!({ "query": "kim" })))
            .await
            .unwrap();
        server
            .run("search_contacts", &fake::args(json!({ "query": "lee" })))
            .await
            .unwrap();

        let queries: Vec<Vec<String>> = google
            .seen("GET", "/v1/people:searchContacts")
            .iter()
            .map(|seen| seen.query("query"))
            .collect();
        assert_eq!(queries, [[""], ["kim"], ["lee"]]);
        assert_eq!(
            found,
            json!({ "results": [{ "name": "김철수", "email": "kim@example.com" }] })
        );
    }

    #[tokio::test]
    async fn the_profile_is_the_signed_in_persons_name_and_address() {
        let google = serve(vec![(
            "GET /v1/people/me",
            fake::json(&json!({
                "names": [{ "displayName": "Mino" }],
                "emailAddresses": [{ "value": "mino@example.com" }],
            })),
        )])
        .await;

        let me = google
            .call(Product::Contacts, "get_user_profile", json!({}))
            .await
            .unwrap();

        assert_eq!(
            google.one("GET", "/v1/people/me").query("personFields"),
            ["names,emailAddresses"]
        );
        assert_eq!(
            me,
            json!({ "name": "Mino", "emailAddress": "mino@example.com" })
        );
    }

    #[tokio::test]
    async fn directory_sources_use_the_people_api_names() {
        let google = serve(vec![(
            "GET /v1/people:searchDirectoryPeople",
            fake::json(&json!({ "people": [] })),
        )])
        .await;

        google
            .call(
                Product::Contacts,
                "search_directory_people",
                json!({ "query": "kim", "sources": ["DOMAIN_CONTACT"] }),
            )
            .await
            .unwrap();

        assert_eq!(
            google
                .one("GET", "/v1/people:searchDirectoryPeople")
                .query("sources"),
            ["DIRECTORY_SOURCE_TYPE_DOMAIN_CONTACT"]
        );
    }
}
