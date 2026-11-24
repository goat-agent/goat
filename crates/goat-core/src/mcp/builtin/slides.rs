use reqwest::{Method, Url};
use serde_json::{Value, json};

use super::args::Args;
use super::drive;
use super::google::{self, Rest, rename};

pub(super) const SCOPES: &[&str] = &[
    "https://www.googleapis.com/auth/drive.readonly",
    "https://www.googleapis.com/auth/drive.file",
    "https://www.googleapis.com/auth/presentations.readonly",
    "https://www.googleapis.com/auth/presentations",
];
pub(super) const TOOLS: &str = include_str!("slides.json");

#[derive(Debug)]
pub(super) struct Slides {
    rest: Rest,
    base: Box<str>,
    drive: Box<str>,
}

impl Slides {
    pub(super) fn new(rest: Rest, base: &str, drive: &str) -> Self {
        Self {
            rest,
            base: base.into(),
            drive: drive.into(),
        }
    }

    pub(super) async fn run(&self, name: &str, args: &Args) -> Result<Value, String> {
        match name {
            "read_presentation" => self.read(args, &[]).await,
            "read_slide_page" => self.read(args, &["pages", &args.text("pageId")?]).await,
            "read_slide_page_thumbnail" => self.thumbnail(args).await,
            "update_presentation" => self.update(args).await,
            _ => Err(google::no_tool(name)),
        }
    }

    async fn read(&self, args: &Args, page: &[&str]) -> Result<Value, String> {
        let id = args.text("presentationId")?;
        let fields = args.list("fields");
        let query = if fields.is_empty() {
            Vec::new()
        } else {
            vec![("fields", fields.join(","))]
        };
        let segments: Vec<&str> = std::iter::once(id.as_str())
            .chain(page.iter().copied())
            .collect();
        let mut content = self.rest.get(self.url(&segments, &query)?).await?;
        if args.flag("commentsIncluded") {
            content["comments"] =
                Value::Array(drive::comments(&self.rest, &self.drive, &id).await?);
        }
        Ok(json!({ "content": content }))
    }

    async fn thumbnail(&self, args: &Args) -> Result<Value, String> {
        let id = args.text("presentationId")?;
        let page = args.text("pageId")?;
        let properties = args
            .value("thumbnailProperties")
            .cloned()
            .unwrap_or_default();
        let mut query = Vec::new();
        for (field, unset) in [
            ("mimeType", "MIME_TYPE_UNSPECIFIED"),
            ("thumbnailSize", "THUMBNAIL_SIZE_UNSPECIFIED"),
        ] {
            if let Some(value) = properties[field].as_str().filter(|value| *value != unset) {
                query.push((
                    match field {
                        "mimeType" => "thumbnailProperties.mimeType",
                        _ => "thumbnailProperties.thumbnailSize",
                    },
                    value.to_owned(),
                ));
            }
        }
        let found = self
            .rest
            .get(self.url(&[&id, "pages", &page, "thumbnail"], &query)?)
            .await?;
        let mut out = json!({ "contentUrl": found["contentUrl"] });
        rename(&found, "width", &mut out, "widthPx");
        rename(&found, "height", &mut out, "heightPx");
        Ok(out)
    }

    async fn update(&self, args: &Args) -> Result<Value, String> {
        let id = args.text("presentationId")?;
        let mut body = json!({ "requests": args.value("requests").cloned().unwrap_or(json!([])) });
        if let Some(control) = args.value("writeControl") {
            body["writeControl"] = control.clone();
        }
        let updated = self
            .rest
            .send(
                Method::POST,
                self.url(&[&format!("{id}:batchUpdate")], &[])?,
                &body,
            )
            .await?;
        Ok(json!({ "replies": updated["replies"] }))
    }

    fn url(&self, segments: &[&str], query: &[(&str, String)]) -> Result<Url, String> {
        let path: Vec<&str> = ["v1", "presentations"]
            .into_iter()
            .chain(segments.iter().copied())
            .collect();
        google::at(&self.base, &path, query)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::super::google::Product;
    use super::super::google::fake::{self, serve};

    #[tokio::test]
    async fn a_thumbnail_reports_its_url_and_size_in_pixels() {
        let google = serve(vec![(
            "GET /v1/presentations/p1/pages/s1/thumbnail",
            fake::json(&json!({ "contentUrl": "https://lh3/x", "width": 800, "height": 450 })),
        )])
        .await;

        let thumbnail = google
            .call(
                Product::Slides,
                "read_slide_page_thumbnail",
                json!({ "presentationId": "p1", "pageId": "s1", "thumbnailProperties": { "thumbnailSize": "MEDIUM", "mimeType": "MIME_TYPE_UNSPECIFIED" } }),
            )
            .await
            .unwrap();

        let seen = google.one("GET", "/v1/presentations/p1/pages/s1/thumbnail");
        assert_eq!(seen.query("thumbnailProperties.thumbnailSize"), ["MEDIUM"]);
        assert_eq!(
            seen.query("thumbnailProperties.mimeType"),
            Vec::<String>::new()
        );
        assert_eq!(
            thumbnail,
            json!({ "contentUrl": "https://lh3/x", "widthPx": 800, "heightPx": 450 })
        );
    }

    #[tokio::test]
    async fn a_page_is_read_with_the_field_mask() {
        let google = serve(vec![(
            "GET /v1/presentations/p1/pages/s1",
            fake::json(&json!({ "objectId": "s1" })),
        )])
        .await;

        let read = google
            .call(
                Product::Slides,
                "read_slide_page",
                json!({ "presentationId": "p1", "pageId": "s1", "fields": ["pageElements.objectId", "objectId"] }),
            )
            .await
            .unwrap();

        assert_eq!(
            google
                .one("GET", "/v1/presentations/p1/pages/s1")
                .query("fields"),
            ["pageElements.objectId,objectId"]
        );
        assert_eq!(read, json!({ "content": { "objectId": "s1" } }));
    }
}
