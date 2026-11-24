use reqwest::{Method, Url};
use serde_json::{Value, json};

use super::args::Args;
use super::drive;
use super::google::{self, Rest};

pub(super) const SCOPES: &[&str] = &[
    "https://www.googleapis.com/auth/drive.readonly",
    "https://www.googleapis.com/auth/drive.file",
    "https://www.googleapis.com/auth/documents.readonly",
    "https://www.googleapis.com/auth/documents",
];
pub(super) const TOOLS: &str = include_str!("docs.json");

#[derive(Debug)]
pub(super) struct Docs {
    rest: Rest,
    base: Box<str>,
    drive: Box<str>,
}

impl Docs {
    pub(super) fn new(rest: Rest, base: &str, drive: &str) -> Self {
        Self {
            rest,
            base: base.into(),
            drive: drive.into(),
        }
    }

    pub(super) async fn run(&self, name: &str, args: &Args) -> Result<Value, String> {
        match name {
            "read_doc" => self.read_doc(args).await,
            "update_doc" => self.update_doc(args).await,
            _ => Err(google::no_tool(name)),
        }
    }

    async fn read_doc(&self, args: &Args) -> Result<Value, String> {
        let id = args.text("documentId")?;
        let mut content = self
            .rest
            .get(self.url(&id, &[("includeTabsContent", "true".to_owned())])?)
            .await?;
        if args.flag("commentsIncluded") {
            content["comments"] =
                Value::Array(drive::comments(&self.rest, &self.drive, &id).await?);
        }
        Ok(json!({ "content": content }))
    }

    async fn update_doc(&self, args: &Args) -> Result<Value, String> {
        let id = args.text("documentId")?;
        let mut body = json!({ "requests": args.value("requests").cloned().unwrap_or(json!([])) });
        if let Some(control) = args.value("writeControl") {
            body["writeControl"] = control.clone();
        }
        self.rest
            .send(
                Method::POST,
                self.url(&format!("{id}:batchUpdate"), &[])?,
                &body,
            )
            .await
    }

    fn url(&self, document: &str, query: &[(&str, String)]) -> Result<Url, String> {
        google::at(&self.base, &["v1", "documents", document], query)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::super::google::Product;
    use super::super::google::fake::{self, serve};

    #[tokio::test]
    async fn a_doc_is_read_with_its_tabs_and_comments_when_asked() {
        let google = serve(vec![
            (
                "GET /v1/documents/d1",
                fake::json(&json!({ "documentId": "d1", "title": "Plan" })),
            ),
            (
                "GET /drive/v3/files/d1/comments",
                fake::json(
                    &json!({ "comments": [{ "id": "c1", "content": "ok", "resolved": true }] }),
                ),
            ),
        ])
        .await;

        let read = google
            .call(
                Product::Docs,
                "read_doc",
                json!({ "documentId": "d1", "commentsIncluded": true }),
            )
            .await
            .unwrap();

        assert_eq!(
            google
                .one("GET", "/v1/documents/d1")
                .query("includeTabsContent"),
            ["true"]
        );
        assert_eq!(read["content"]["title"], "Plan");
        assert_eq!(read["content"]["comments"][0]["status"], "RESOLVED");
    }

    #[tokio::test]
    async fn an_update_is_sent_as_one_batch() {
        let google = serve(vec![(
            "POST /v1/documents/d1:batchUpdate",
            fake::json(&json!({ "documentId": "d1", "replies": [{}] })),
        )])
        .await;
        let requests = json!([{ "insertText": { "text": "안녕", "location": { "index": 1 } } }]);

        let updated = google
            .call(
                Product::Docs,
                "update_doc",
                json!({ "documentId": "d1", "requests": requests }),
            )
            .await
            .unwrap();

        assert_eq!(
            google.one("POST", "/v1/documents/d1:batchUpdate").json(),
            json!({ "requests": requests })
        );
        assert_eq!(updated["documentId"], "d1");
    }
}
