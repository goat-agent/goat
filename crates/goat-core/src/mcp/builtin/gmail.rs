use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use futures::future::join_all;
use reqwest::{Method, Url};
use serde_json::{Value, json};

use super::args::Args;
use super::google::{self, Rest, copy, items};
use super::mime::{self, Attachment, Draft};

pub(super) const SCOPES: &[&str] = &["https://www.googleapis.com/auth/gmail.modify"];
pub(super) const TOOLS: &str = include_str!("gmail.json");
const PAGE: u64 = 20;
const MOST_PAGE: u64 = 50;
const SEARCH_MESSAGES: usize = 5;
const ATTACHMENT_BYTES: usize = 25 * 1024 * 1024;
const VIEW: &str = "https://mail.google.com/mail/#all/";
const DRAFT_VIEW: &str = "https://mail.google.com/mail/#drafts?compose=";
const HEADERS: &[&str] = &["From", "To", "Cc", "Bcc", "Subject"];
const PRESETS: &[(&str, &str, &str)] = &[
    ("LABEL_COLOR_PRESET_BLACK", "#000000", "#ffffff"),
    ("LABEL_COLOR_PRESET_DARK_GRAY", "#434343", "#ffffff"),
    ("LABEL_COLOR_PRESET_GRAY", "#999999", "#ffffff"),
    ("LABEL_COLOR_PRESET_LIGHT_GRAY", "#cccccc", "#000000"),
    ("LABEL_COLOR_PRESET_WHITE", "#ffffff", "#000000"),
    ("LABEL_COLOR_PRESET_RED", "#fb4c2f", "#ffffff"),
    ("LABEL_COLOR_PRESET_ORANGE", "#ffad47", "#ffffff"),
    ("LABEL_COLOR_PRESET_YELLOW", "#fad165", "#000000"),
    ("LABEL_COLOR_PRESET_GREEN", "#16a766", "#ffffff"),
    ("LABEL_COLOR_PRESET_MINT", "#43d692", "#ffffff"),
    ("LABEL_COLOR_PRESET_TEAL", "#2da2bb", "#ffffff"),
    ("LABEL_COLOR_PRESET_BLUE", "#4a86e8", "#ffffff"),
    ("LABEL_COLOR_PRESET_PURPLE", "#a479e2", "#ffffff"),
    ("LABEL_COLOR_PRESET_PINK", "#f691b3", "#ffffff"),
    ("LABEL_COLOR_PRESET_DARK_RED", "#ac2b16", "#ffffff"),
    ("LABEL_COLOR_PRESET_DARK_ORANGE", "#cf8933", "#ffffff"),
    ("LABEL_COLOR_PRESET_DARK_GREEN", "#0b804b", "#ffffff"),
    ("LABEL_COLOR_PRESET_DARK_BLUE", "#285bac", "#ffffff"),
    ("LABEL_COLOR_PRESET_DARK_PURPLE", "#653e9b", "#ffffff"),
    ("LABEL_COLOR_PRESET_DARK_PINK", "#b65775", "#ffffff"),
    ("LABEL_COLOR_PRESET_BROWN", "#7a4706", "#ffffff"),
];

#[derive(Debug)]
pub(super) struct Gmail {
    rest: Rest,
    base: Box<str>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Format {
    Minimal,
    Metadata,
    Plain,
    Full,
    Raw,
}

impl Gmail {
    pub(super) fn new(rest: Rest, base: &str) -> Self {
        Self {
            rest,
            base: base.into(),
        }
    }

    pub(super) async fn run(&self, name: &str, args: &Args) -> Result<Value, String> {
        match name {
            "search_threads" => self.search_threads(args).await,
            "get_thread" => self.get_thread(args).await,
            "get_message" => self.get_message(args).await,
            "list_drafts" => self.list_drafts(args).await,
            "get_draft" => self.get_draft(args).await,
            "create_draft" => self.create_draft(args).await,
            "list_labels" => self.list_labels().await,
            "create_label" => self.create_label(args).await,
            "label_thread" => {
                self.relabel(
                    "threads",
                    &args.text("threadId")?,
                    &args.list("labelIds"),
                    &[],
                )
                .await
            }
            "unlabel_thread" => {
                self.relabel(
                    "threads",
                    &args.text("threadId")?,
                    &[],
                    &args.list("labelIds"),
                )
                .await
            }
            "label_message" => {
                self.relabel(
                    "messages",
                    &args.text("messageId")?,
                    &args.list("labelIds"),
                    &[],
                )
                .await
            }
            "unlabel_message" => {
                self.relabel(
                    "messages",
                    &args.text("messageId")?,
                    &[],
                    &args.list("labelIds"),
                )
                .await
            }
            "update_message_labels" => {
                let id = args.text("messageId")?;
                let changed = self
                    .post(
                        &format!("messages/{id}/modify"),
                        &json!({ "addLabelIds": args.list("addLabelIds"), "removeLabelIds": args.list("removeLabelIds") }),
                    )
                    .await?;
                Ok(json!({ "messageId": id, "labelIds": changed["labelIds"] }))
            }
            "apply_sensitive_thread_label" => {
                self.sensitive("threads", &args.text("threadId")?, args)
                    .await
            }
            "apply_sensitive_message_label" => {
                self.sensitive("messages", &args.text("messageId")?, args)
                    .await
            }
            "trash_thread" => self.act("threads", &args.text("threadId")?, "trash").await,
            "untrash_thread" => {
                self.act("threads", &args.text("threadId")?, "untrash")
                    .await
            }
            "trash_message" => {
                self.act("messages", &args.text("messageId")?, "trash")
                    .await
            }
            "untrash_message" => {
                self.act("messages", &args.text("messageId")?, "untrash")
                    .await
            }
            "mark_thread_spam" => {
                self.relabel("threads", &args.text("threadId")?, &spam(), &inbox())
                    .await
            }
            "unmark_thread_spam" => {
                self.relabel("threads", &args.text("threadId")?, &inbox(), &spam())
                    .await
            }
            "mark_message_spam" => {
                self.relabel("messages", &args.text("messageId")?, &spam(), &inbox())
                    .await
            }
            "unmark_message_spam" => {
                self.relabel("messages", &args.text("messageId")?, &inbox(), &spam())
                    .await
            }
            _ => Err(google::no_tool(name)),
        }
    }

    async fn search_threads(&self, args: &Args) -> Result<Value, String> {
        let view = args.word("view").unwrap_or("THREAD_VIEW_MINIMAL");
        let mut query = vec![(
            "maxResults",
            args.page("pageSize", PAGE, MOST_PAGE).to_string(),
        )];
        if let Some(text) = args.word("query") {
            query.push(("q", text.to_owned()));
        }
        if let Some(token) = args.word("pageToken") {
            query.push(("pageToken", token.to_owned()));
        }
        if args.flag("includeTrash") {
            query.push(("includeSpamTrash", "true".to_owned()));
        }
        let listed = self.get("threads", &query).await?;
        let ids: Vec<String> = items(&listed, "threads")
            .filter_map(|thread| thread["id"].as_str().map(str::to_owned))
            .collect();
        let threads: Vec<Value> = if view == "THREAD_VIEW_IDS_ONLY" {
            ids.iter().map(|id| json!({ "id": id })).collect()
        } else {
            let format = if view == "THREAD_VIEW_METADATA_ONLY" {
                Format::Metadata
            } else {
                Format::Minimal
            };
            let latest = if view == "THREAD_VIEW_SUMMARY" {
                1
            } else {
                SEARCH_MESSAGES
            };
            join_all(ids.iter().map(|id| self.thread(id, format, latest)))
                .await
                .into_iter()
                .collect::<Result<_, _>>()?
        };
        let mut out =
            json!({ "threads": threads, "resultCountEstimate": listed["resultSizeEstimate"] });
        copy(&listed, &mut out, "nextPageToken");
        Ok(out)
    }

    async fn get_thread(&self, args: &Args) -> Result<Value, String> {
        let format = Format::of(args.word("messageFormat"), Format::Plain);
        self.thread(&args.text("threadId")?, format, usize::MAX)
            .await
    }

    async fn thread(&self, id: &str, format: Format, latest: usize) -> Result<Value, String> {
        let found = self.get(&format!("threads/{id}"), &format.query()).await?;
        let kept: Vec<&Value> = items(&found, "messages")
            .filter(|message| !labels(message).any(|label| label == "DRAFT"))
            .collect();
        let shown = &kept[kept.len().saturating_sub(latest)..];
        let mut messages = Vec::with_capacity(shown.len());
        for message in shown {
            messages.push(self.message(message, format).await?);
        }
        Ok(json!({
            "id": id,
            "messageCount": kept.len(),
            "viewUrl": format!("{VIEW}{id}"),
            "messages": messages,
        }))
    }

    async fn get_message(&self, args: &Args) -> Result<Value, String> {
        let id = args.text("messageId")?;
        let format = Format::of(args.word("messageFormat"), Format::Full);
        let found = self.get(&format!("messages/{id}"), &format.query()).await?;
        let mut out = self.message(&found, format).await?;
        for field in ["historyId", "internalDate", "sizeEstimate"] {
            copy(&found, &mut out, field);
        }
        Ok(out)
    }

    async fn list_drafts(&self, args: &Args) -> Result<Value, String> {
        let format = if args.word("view") == Some("DRAFT_VIEW_FULL") {
            Format::Plain
        } else {
            Format::Metadata
        };
        let mut query = vec![(
            "maxResults",
            args.page("pageSize", PAGE, MOST_PAGE).to_string(),
        )];
        if let Some(text) = args.word("query") {
            query.push(("q", text.to_owned()));
        }
        if let Some(token) = args.word("pageToken") {
            query.push(("pageToken", token.to_owned()));
        }
        let listed = self.get("drafts", &query).await?;
        let ids: Vec<String> = items(&listed, "drafts")
            .filter_map(|draft| draft["id"].as_str().map(str::to_owned))
            .collect();
        let drafts: Vec<Value> = join_all(ids.iter().map(|id| self.draft(id, format)))
            .await
            .into_iter()
            .collect::<Result<_, _>>()?;
        let mut out = json!({ "drafts": drafts });
        copy(&listed, &mut out, "nextPageToken");
        Ok(out)
    }

    async fn get_draft(&self, args: &Args) -> Result<Value, String> {
        let format = Format::of(args.word("messageFormat"), Format::Full);
        self.draft(&args.text("draftId")?, format).await
    }

    async fn draft(&self, id: &str, format: Format) -> Result<Value, String> {
        let found = self.get(&format!("drafts/{id}"), &format.query()).await?;
        let message = &found["message"];
        let mut out = self.message(message, format).await?;
        out["messageId"] = message["id"].clone();
        out["id"] = Value::String(id.to_owned());
        out["viewUrl"] = Value::String(format!(
            "{DRAFT_VIEW}{}",
            message["id"].as_str().unwrap_or_default()
        ));
        Ok(out)
    }

    async fn create_draft(&self, args: &Args) -> Result<Value, String> {
        let mut draft = Draft {
            to: args.list("to"),
            cc: args.list("cc"),
            bcc: args.list("bcc"),
            subject: args.word("subject").unwrap_or_default().to_owned(),
            text: args.word("body").map(str::to_owned),
            html: args.word("htmlBody").map(str::to_owned),
            attachments: attachments(args)?,
            ..Draft::default()
        };
        let mut thread = None;
        if let Some(original) = args.word("replyToMessageId") {
            let found = self
                .get(
                    &format!("messages/{original}"),
                    &[
                        ("format", "metadata".to_owned()),
                        ("metadataHeaders", "Message-ID".to_owned()),
                        ("metadataHeaders", "References".to_owned()),
                        ("metadataHeaders", "Subject".to_owned()),
                    ],
                )
                .await?;
            let message_id = header(&found, "Message-ID");
            if let Some(id) = &message_id {
                let earlier = header(&found, "References").unwrap_or_default();
                draft.references = Some(format!("{earlier} {id}").trim().to_owned());
                draft.in_reply_to = Some(id.clone());
            }
            if draft.subject.is_empty() {
                let subject = header(&found, "Subject").unwrap_or_default();
                draft.subject = if subject.to_lowercase().starts_with("re:") {
                    subject
                } else {
                    format!("Re: {subject}")
                };
            }
            thread = found["threadId"].as_str().map(str::to_owned);
        }
        if args.flag("includeSignature") {
            self.sign(&mut draft).await?;
        }
        let raw = URL_SAFE_NO_PAD.encode(draft.build()?);
        let mut message = json!({ "raw": raw });
        if let Some(thread) = thread {
            message["threadId"] = Value::String(thread);
        }
        let created = self.post("drafts", &json!({ "message": message })).await?;
        let id = created["id"].as_str().unwrap_or_default();
        self.draft(id, Format::Minimal).await
    }

    async fn sign(&self, draft: &mut Draft) -> Result<(), String> {
        let found = self.get("settings/sendAs", &[]).await?;
        let Some(signature) = items(&found, "sendAs")
            .find(|alias| alias["isDefault"].as_bool() == Some(true))
            .and_then(|alias| alias["signature"].as_str())
            .filter(|signature| !signature.is_empty())
        else {
            return Ok(());
        };
        if let Some(html) = &mut draft.html {
            html.push_str("<br><br>");
            html.push_str(signature);
        }
        if let Some(text) = &mut draft.text {
            text.push_str("\n\n-- \n");
            text.push_str(&plain(signature));
        }
        Ok(())
    }

    async fn list_labels(&self) -> Result<Value, String> {
        let found = self.get("labels", &[]).await?;
        Ok(json!({ "labels": items(&found, "labels").map(label).collect::<Vec<_>>() }))
    }

    async fn create_label(&self, args: &Args) -> Result<Value, String> {
        let name = args.text("displayName")?;
        if args.flag("autoCreateParentLabels") {
            let existing = self.get("labels", &[]).await?;
            let known: Vec<&str> = items(&existing, "labels")
                .filter_map(|label| label["name"].as_str())
                .collect();
            let mut parent = String::new();
            for part in name.split('/').take(name.split('/').count() - 1) {
                if !parent.is_empty() {
                    parent.push('/');
                }
                parent.push_str(part);
                if !known.contains(&parent.as_str()) {
                    self.post("labels", &json!({ "name": parent })).await?;
                }
            }
        }
        let mut body = json!({ "name": name });
        if let Some(shown) = args.word("labelListVisibility").and_then(label_list) {
            body["labelListVisibility"] = Value::String(shown.to_owned());
        }
        if let Some(shown) = args.word("messageListVisibility").and_then(message_list) {
            body["messageListVisibility"] = Value::String(shown.to_owned());
        }
        if let Some((_, background, text)) = args
            .word("colorPreset")
            .and_then(|wanted| PRESETS.iter().find(|(preset, _, _)| *preset == wanted))
        {
            body["color"] = json!({ "backgroundColor": background, "textColor": text });
        } else if let Some(color) = args.value("color").filter(|color| color.is_object()) {
            body["color"] = color.clone();
        }
        let created = self.post("labels", &body).await?;
        Ok(label(&created))
    }

    async fn relabel(
        &self,
        kind: &str,
        id: &str,
        add: &[String],
        remove: &[String],
    ) -> Result<Value, String> {
        self.post(
            &format!("{kind}/{id}/modify"),
            &json!({ "addLabelIds": add, "removeLabelIds": remove }),
        )
        .await?;
        Ok(json!({}))
    }

    async fn sensitive(&self, kind: &str, id: &str, args: &Args) -> Result<Value, String> {
        match args.word("labelOption") {
            Some("TRASH") => self.act(kind, id, "trash").await,
            Some("SPAM") => self.relabel(kind, id, &spam(), &inbox()).await,
            _ => Err("labelOption must be TRASH or SPAM".to_owned()),
        }
    }

    async fn act(&self, kind: &str, id: &str, action: &str) -> Result<Value, String> {
        self.post(&format!("{kind}/{id}/{action}"), &json!({}))
            .await?;
        Ok(json!({}))
    }

    async fn message(&self, found: &Value, format: Format) -> Result<Value, String> {
        let id = found["id"].as_str().unwrap_or_default();
        let mut out = json!({
            "id": id,
            "threadId": found["threadId"],
            "labelIds": found["labelIds"],
            "viewUrl": format!("{VIEW}{id}"),
        });
        if let Some(date) = found["internalDate"]
            .as_str()
            .and_then(|ms| ms.parse::<i64>().ok())
        {
            out["date"] = Value::String(rfc3339(date / 1000));
        }
        if format == Format::Raw {
            let bytes = decode(found["raw"].as_str().unwrap_or_default());
            out["raw"] = Value::String(String::from_utf8_lossy(&bytes).into_owned());
            return Ok(out);
        }
        let mut names = Vec::new();
        for (field, header_name) in [
            ("sender", "From"),
            ("toRecipients", "To"),
            ("ccRecipients", "Cc"),
            ("bccRecipients", "Bcc"),
        ] {
            let listed =
                header(found, header_name).map_or_else(Vec::new, |raw| mime::addresses(&raw));
            let mut emails = Vec::with_capacity(listed.len());
            for entry in listed {
                let (name, email) = mime::display_name(&entry);
                if let Some(name) = name {
                    names.push(json!({ "displayName": name, "emailAddress": email }));
                }
                emails.push(email);
            }
            out[field] = if field == "sender" {
                emails.into_iter().next().map_or(Value::Null, Value::String)
            } else {
                json!(emails)
            };
        }
        if !names.is_empty() {
            out["addressDisplayNames"] = json!(names);
        }
        if format == Format::Metadata {
            return Ok(out);
        }
        out["subject"] = header(found, "Subject").map_or(Value::Null, Value::String);
        copy(found, &mut out, "snippet");
        if format == Format::Minimal {
            return Ok(out);
        }
        let mut body = Body::default();
        self.walk(id, &found["payload"], "", &mut body).await?;
        let text = body.text.or_else(|| body.html.as_deref().map(plain));
        out["plaintextBody"] = text.map_or(Value::Null, Value::String);
        if format == Format::Full {
            out["htmlBody"] = body.html.map_or(Value::Null, Value::String);
        }
        out["attachmentIds"] = json!(
            body.files
                .iter()
                .map(|file| file["id"].clone())
                .collect::<Vec<_>>()
        );
        out["attachments"] = json!(body.files);
        Ok(out)
    }

    async fn walk(
        &self,
        message: &str,
        part: &Value,
        at: &str,
        body: &mut Body,
    ) -> Result<(), String> {
        let mime_type = part["mimeType"].as_str().unwrap_or_default();
        let filename = part["filename"].as_str().unwrap_or_default();
        let attachment = part["body"]["attachmentId"].as_str();
        if !filename.is_empty() {
            body.files.push(json!({
                "id": attachment,
                "filename": filename,
                "mimeType": mime_type,
                "partId": at,
            }));
            return Ok(());
        }
        if mime_type == "text/plain" || mime_type == "text/html" {
            let bytes = match (part["body"]["data"].as_str(), attachment) {
                (Some(data), _) => decode(data),
                (None, Some(attachment)) => {
                    let found = self
                        .get(&format!("messages/{message}/attachments/{attachment}"), &[])
                        .await?;
                    decode(found["data"].as_str().unwrap_or_default())
                }
                (None, None) => Vec::new(),
            };
            let text = in_charset(&bytes, part);
            let slot = if mime_type == "text/plain" {
                &mut body.text
            } else {
                &mut body.html
            };
            if slot.is_none() {
                *slot = Some(text);
            }
            return Ok(());
        }
        for (index, child) in items(part, "parts").enumerate() {
            let child_at = if at.is_empty() {
                index.to_string()
            } else {
                format!("{at}.{index}")
            };
            Box::pin(self.walk(message, child, &child_at, body)).await?;
        }
        Ok(())
    }

    async fn get(&self, path: &str, query: &[(&str, String)]) -> Result<Value, String> {
        self.rest.get(self.url(path, query)?).await
    }

    async fn post(&self, path: &str, body: &Value) -> Result<Value, String> {
        self.rest
            .send(Method::POST, self.url(path, &[])?, body)
            .await
    }

    fn url(&self, path: &str, query: &[(&str, String)]) -> Result<Url, String> {
        let segments: Vec<&str> = ["gmail", "v1", "users", "me"]
            .into_iter()
            .chain(path.split('/'))
            .collect();
        google::at(&self.base, &segments, query)
    }
}

impl Format {
    fn of(raw: Option<&str>, default: Self) -> Self {
        match raw {
            Some("MINIMAL") => Self::Minimal,
            Some("METADATA_ONLY") => Self::Metadata,
            Some("PLAIN_TEXT") => Self::Plain,
            Some("FULL_CONTENT") => Self::Full,
            Some("RAW") => Self::Raw,
            _ => default,
        }
    }

    fn query(self) -> Vec<(&'static str, String)> {
        match self {
            Self::Minimal | Self::Metadata => std::iter::once(("format", "metadata".to_owned()))
                .chain(
                    HEADERS
                        .iter()
                        .map(|name| ("metadataHeaders", (*name).to_owned())),
                )
                .collect(),
            Self::Plain | Self::Full => vec![("format", "full".to_owned())],
            Self::Raw => vec![("format", "raw".to_owned())],
        }
    }
}

#[derive(Default)]
struct Body {
    text: Option<String>,
    html: Option<String>,
    files: Vec<Value>,
}

fn attachments(args: &Args) -> Result<Vec<Attachment>, String> {
    let mut total = 0;
    let mut found = Vec::new();
    for item in args
        .0
        .get("attachments")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let content = item["content"]
            .as_str()
            .ok_or("an attachment needs content")?;
        let bytes = STANDARD
            .decode(content.trim())
            .or_else(|_| URL_SAFE_NO_PAD.decode(content.trim().trim_end_matches('=')))
            .map_err(|_| "an attachment's content is not base64".to_owned())?;
        total += bytes.len();
        if total > ATTACHMENT_BYTES {
            return Err("attachments may not exceed 25MB together".to_owned());
        }
        found.push(Attachment {
            name: item["filename"].as_str().map(str::to_owned),
            mime: item["mimeType"]
                .as_str()
                .unwrap_or("application/octet-stream")
                .to_owned(),
            bytes,
            is_inline: item["inline"].as_bool() == Some(true),
        });
    }
    Ok(found)
}

fn label(found: &Value) -> Value {
    let background = found["color"]["backgroundColor"].as_str();
    let text = found["color"]["textColor"].as_str();
    let preset = PRESETS
        .iter()
        .find(|(_, b, t)| Some(*b) == background && Some(*t) == text)
        .map_or("LABEL_COLOR_PRESET_UNSPECIFIED", |(preset, _, _)| preset);
    let mut out = json!({
        "labelId": found["id"],
        "name": found["name"],
        "labelType": found["type"].as_str().map(str::to_uppercase),
        "colorPreset": preset,
    });
    if found["color"].is_object() {
        out["color"] = found["color"].clone();
    }
    if let Some(shown) = found["labelListVisibility"].as_str() {
        out["labelListVisibility"] = Value::String(
            match shown {
                "labelShow" => "LABEL_SHOW",
                "labelShowIfUnread" => "LABEL_SHOW_IF_UNREAD",
                "labelHide" => "LABEL_HIDE",
                other => other,
            }
            .to_owned(),
        );
    }
    if let Some(shown) = found["messageListVisibility"].as_str() {
        out["messageListVisibility"] = Value::String(shown.to_uppercase());
    }
    for field in [
        "messagesTotal",
        "messagesUnread",
        "threadsTotal",
        "threadsUnread",
    ] {
        copy(found, &mut out, field);
    }
    out
}

fn label_list(raw: &str) -> Option<&'static str> {
    match raw {
        "LABEL_SHOW" => Some("labelShow"),
        "LABEL_SHOW_IF_UNREAD" => Some("labelShowIfUnread"),
        "LABEL_HIDE" => Some("labelHide"),
        _ => None,
    }
}

fn message_list(raw: &str) -> Option<&'static str> {
    match raw {
        "SHOW" => Some("show"),
        "HIDE" => Some("hide"),
        _ => None,
    }
}

fn labels(message: &Value) -> impl Iterator<Item = &str> {
    items(message, "labelIds").filter_map(Value::as_str)
}

fn header(message: &Value, name: &str) -> Option<String> {
    items(&message["payload"], "headers")
        .find(|header| {
            header["name"]
                .as_str()
                .is_some_and(|found| found.eq_ignore_ascii_case(name))
        })
        .and_then(|header| header["value"].as_str())
        .map(str::to_owned)
}

fn spam() -> Vec<String> {
    vec!["SPAM".to_owned()]
}

fn inbox() -> Vec<String> {
    vec!["INBOX".to_owned()]
}

fn decode(data: &str) -> Vec<u8> {
    URL_SAFE_NO_PAD
        .decode(data.trim_end_matches('='))
        .unwrap_or_default()
}

fn in_charset(bytes: &[u8], part: &Value) -> String {
    let charset = items(part, "headers")
        .find(|header| {
            header["name"]
                .as_str()
                .is_some_and(|name| name.eq_ignore_ascii_case("Content-Type"))
        })
        .and_then(|header| header["value"].as_str())
        .and_then(|value| {
            let lower = value.to_ascii_lowercase();
            let at = lower.find("charset=")? + "charset=".len();
            let rest = &value[at..];
            Some(
                rest.split(';')
                    .next()
                    .unwrap_or_default()
                    .trim()
                    .trim_matches('"')
                    .to_owned(),
            )
        });
    let encoding = charset
        .as_deref()
        .and_then(|label| encoding_rs::Encoding::for_label(label.as_bytes()))
        .unwrap_or(encoding_rs::UTF_8);
    encoding.decode(bytes).0.into_owned()
}

fn plain(html: &str) -> String {
    htmd::convert(html).unwrap_or_else(|_| html.to_owned())
}

fn rfc3339(seconds: i64) -> String {
    jiff::Timestamp::from_second(seconds).map_or_else(|_| seconds.to_string(), |at| at.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_are_rfc3339_in_utc() {
        assert_eq!(rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339(1_795_867_200), "2026-11-28T12:00:00Z");
    }

    #[test]
    fn a_legacy_korean_charset_is_decoded() {
        let (bytes, _, _) = encoding_rs::EUC_KR.encode("안녕");
        let part = json!({ "headers": [{ "name": "Content-Type", "value": "text/plain; charset=\"euc-kr\"" }] });

        assert_eq!(in_charset(&bytes, &part), "안녕");
    }
}
