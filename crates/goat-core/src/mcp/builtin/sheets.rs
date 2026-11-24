use reqwest::{Method, Url};
use serde_json::{Value, json};

use super::drive;
use super::google::{self, Args, Rest};

pub(super) const SCOPES: &[&str] = &[
    "https://www.googleapis.com/auth/drive.readonly",
    "https://www.googleapis.com/auth/drive.file",
    "https://www.googleapis.com/auth/spreadsheets.readonly",
    "https://www.googleapis.com/auth/spreadsheets",
];
pub(super) const TOOLS: &str = include_str!("sheets.json");
const DONE: &str = "SUCCESS";

#[derive(Debug)]
pub(super) struct Sheets {
    rest: Rest,
    base: Box<str>,
    drive: Box<str>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Input {
    Raw,
    Formulas,
}

impl Sheets {
    pub(super) fn new(rest: Rest, base: &str, drive: &str) -> Self {
        Self {
            rest,
            base: base.into(),
            drive: drive.into(),
        }
    }

    pub(super) async fn run(&self, name: &str, args: &Args) -> Result<Value, String> {
        match name {
            "get_values" => self.get_values(args).await,
            "get_spreadsheet" => self.get_spreadsheet(args).await,
            "update_spreadsheet" => {
                let mut body =
                    json!({ "requests": args.value("requests").cloned().unwrap_or(json!([])) });
                if let Some(control) = args.value("writeControl") {
                    body["writeControl"] = control.clone();
                }
                self.batch(args, &body).await
            }
            "update_values" => self.update_values(args, "values", Input::Raw).await,
            "update_formulas" => self.update_values(args, "formulas", Input::Formulas).await,
            "insert_dimension" => self.insert_dimension(args).await,
            _ => Err(google::no_tool(name)),
        }
    }

    async fn get_values(&self, args: &Args) -> Result<Value, String> {
        let id = args.text("spreadsheetId")?;
        let range = args.text("range")?;
        let found = self
            .rest
            .get(self.url(&[&id, "values", &range], &[])?)
            .await?;
        Ok(json!({ "range": found["range"], "values": found["values"] }))
    }

    async fn get_spreadsheet(&self, args: &Args) -> Result<Value, String> {
        let id = args.text("spreadsheetId")?;
        let mut query: Vec<(&str, String)> = args
            .list("ranges")
            .into_iter()
            .map(|range| ("ranges", range))
            .collect();
        if args.flag("includeGridData") {
            query.push(("includeGridData", "true".to_owned()));
        }
        let fields = args.list("fields");
        if !fields.is_empty() {
            query.push(("fields", fields.join(",")));
        }
        let mut content = self.rest.get(self.url(&[&id], &query)?).await?;
        if args.flag("commentsIncluded") {
            content["comments"] =
                Value::Array(drive::comments(&self.rest, &self.drive, &id).await?);
        }
        Ok(json!({ "content": content }))
    }

    async fn update_values(&self, args: &Args, field: &str, input: Input) -> Result<Value, String> {
        let id = args.text("spreadsheetId")?;
        let range = args.text("range")?;
        let option = match input {
            Input::Raw => "RAW",
            Input::Formulas => "USER_ENTERED",
        };
        let updated = self
            .rest
            .send(
                Method::PUT,
                self.url(
                    &[&id, "values", &range],
                    &[("valueInputOption", option.to_owned())],
                )?,
                &json!({ "range": range, "values": args.value(field).cloned().unwrap_or(json!([])) }),
            )
            .await?;
        Ok(done(updated))
    }

    async fn insert_dimension(&self, args: &Args) -> Result<Value, String> {
        let mut insert = json!({
            "range": {
                "sheetId": args.value("sheetId"),
                "dimension": args.text("dimension")?,
                "startIndex": args.value("startIndex"),
                "endIndex": args.value("endIndex"),
            },
        });
        if args.has("inheritFromBefore") {
            insert["inheritFromBefore"] = Value::Bool(args.flag("inheritFromBefore"));
        }
        self.batch(
            args,
            &json!({ "requests": [{ "insertDimension": insert }] }),
        )
        .await
    }

    async fn batch(&self, args: &Args, body: &Value) -> Result<Value, String> {
        let id = args.text("spreadsheetId")?;
        let updated = self
            .rest
            .send(
                Method::POST,
                self.url(&[&format!("{id}:batchUpdate")], &[])?,
                body,
            )
            .await?;
        Ok(done(updated))
    }

    fn url(&self, segments: &[&str], query: &[(&str, String)]) -> Result<Url, String> {
        let path: Vec<&str> = ["v4", "spreadsheets"]
            .into_iter()
            .chain(segments.iter().copied())
            .collect();
        google::at(&self.base, &path, query)
    }
}

fn done(mut response: Value) -> Value {
    if response.is_object() {
        response["status"] = Value::String(DONE.to_owned());
    }
    json!({ "content": response })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::super::google::Product;
    use super::super::google::fake::{self, serve};

    #[tokio::test]
    async fn formulas_are_entered_as_a_user_would_and_values_are_kept_raw() {
        let google = serve(vec![(
            "PUT /v4/spreadsheets/s1/values/",
            fake::json(&json!({ "updatedCells": 1 })),
        )])
        .await;

        let updated = google
            .call(
                Product::Sheets,
                "update_formulas",
                json!({ "spreadsheetId": "s1", "range": "Sheet1!C1", "formulas": [["=A1+B1"]] }),
            )
            .await
            .unwrap();
        google
            .call(
                Product::Sheets,
                "update_values",
                json!({ "spreadsheetId": "s1", "range": "Sheet1!A1", "values": [["=A1"]] }),
            )
            .await
            .unwrap();

        let formulas = google.one("PUT", "/v4/spreadsheets/s1/values/Sheet1!C1");
        let values = google.one("PUT", "/v4/spreadsheets/s1/values/Sheet1!A1");
        assert_eq!(formulas.query("valueInputOption"), ["USER_ENTERED"]);
        assert_eq!(formulas.json()["values"], json!([["=A1+B1"]]));
        assert_eq!(values.query("valueInputOption"), ["RAW"]);
        assert_eq!(
            updated,
            json!({ "content": { "updatedCells": 1, "status": "SUCCESS" } })
        );
    }

    #[tokio::test]
    async fn rows_are_inserted_through_a_batch_update() {
        let google = serve(vec![(
            "POST /v4/spreadsheets/s1:batchUpdate",
            fake::json(&json!({ "spreadsheetId": "s1", "replies": [{}] })),
        )])
        .await;

        google
            .call(
                Product::Sheets,
                "insert_dimension",
                json!({ "spreadsheetId": "s1", "sheetId": 0, "dimension": "ROWS", "startIndex": 1, "endIndex": 3 }),
            )
            .await
            .unwrap();

        assert_eq!(
            google.one("POST", "/v4/spreadsheets/s1:batchUpdate").json(),
            json!({ "requests": [{ "insertDimension": {
                "range": { "sheetId": 0, "dimension": "ROWS", "startIndex": 1, "endIndex": 3 },
            }}]})
        );
    }

    #[tokio::test]
    async fn a_range_is_read_as_rows() {
        let google = serve(vec![(
            "GET /v4/spreadsheets/s1/values/",
            fake::json(&json!({ "range": "Sheet1!A1:B2", "majorDimension": "ROWS", "values": [["a", "1"]] })),
        )])
        .await;

        let read = google
            .call(
                Product::Sheets,
                "get_values",
                json!({ "spreadsheetId": "s1", "range": "Sheet1!A1:B2" }),
            )
            .await
            .unwrap();

        assert_eq!(
            read,
            json!({ "range": "Sheet1!A1:B2", "values": [["a", "1"]] })
        );
    }
}
