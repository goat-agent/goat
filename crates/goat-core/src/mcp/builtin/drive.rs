use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use reqwest::{Method, Url};
use serde_json::{Value, json};

use super::args::Args;
use super::google::{self, Rest, copy, items, rename};
use crate::oauth;

pub(super) const SCOPES: &[&str] = &[
    "https://www.googleapis.com/auth/drive.readonly",
    "https://www.googleapis.com/auth/drive.file",
];
pub(super) const TOOLS: &str = include_str!("drive.json");
const FIELDS: &str = "id,name,mimeType,description,createdTime,modifiedTime,viewedByMeTime,\
sharedWithMeTime,starred,trashed,fileExtension,size,parents,owners(emailAddress,displayName),\
lastModifyingUser(emailAddress,displayName),webViewLink,driveId,writersCanShare,\
capabilities(canAddChildren,canEdit,canMoveItemWithinDrive,canTrash),\
contentRestrictions(readOnly,reason),shortcutDetails(targetId,targetMimeType),\
inheritedPermissionsDisabled";
const COMMENT_FIELDS: &str = "comments(id,content,resolved,modifiedTime,author(displayName),\
replies(id,content,modifiedTime,author(displayName)))";
const PAGE: u64 = 10;
const MOST_PAGE: u64 = 100;
const COMMENTS: &str = "100";
const DOWNLOAD_BYTES: usize = 10 * 1024 * 1024;
const DOCUMENT: &str = "application/vnd.google-apps.document";
const SPREADSHEET: &str = "application/vnd.google-apps.spreadsheet";
const PRESENTATION: &str = "application/vnd.google-apps.presentation";
const DRAWING: &str = "application/vnd.google-apps.drawing";
const NATIVE: &str = "application/vnd.google-apps.";
const UNSUPPORTED_TERMS: &[&str] = &[
    "ancestors",
    "creator",
    "sharedFrom",
    "sharedTo",
    "followUp",
    "folderProperties",
];

#[derive(Debug)]
pub(super) struct Drive {
    rest: Rest,
    base: Box<str>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Reading {
    Export(&'static str),
    Media,
    Convert(&'static str),
}

impl Drive {
    pub(super) fn new(rest: Rest, base: &str) -> Self {
        Self {
            rest,
            base: base.into(),
        }
    }

    pub(super) async fn run(&self, name: &str, args: &Args) -> Result<Value, String> {
        match name {
            "search_files" => self.search_files(args).await,
            "list_recent_files" => self.list_recent_files(args).await,
            "get_file_metadata" => self.get_file_metadata(args).await,
            "get_file_permissions" => self.get_file_permissions(args).await,
            "read_file_content" => self.read_file_content(args).await,
            "download_file_content" => self.download_file_content(args).await,
            "create_file" => self.create_file(args).await,
            "copy_file" => self.copy_file(args).await,
            "upload_file_revision" => self.upload_file_revision(args).await,
            _ => Err(google::no_tool(name)),
        }
    }

    async fn search_files(&self, args: &Args) -> Result<Value, String> {
        let query = translate(args.word("query").unwrap_or_default())?;
        let mut q = "trashed = false".to_owned();
        if !query.is_empty() {
            q = if query.split_whitespace().any(|word| word == "trashed") {
                query
            } else {
                format!("({query}) and {q}")
            };
        }
        let mut wanted = vec![("q", q)];
        if let Some(order) = args.word("orderBy") {
            wanted.push(("orderBy", search_order(order)?));
        }
        self.list(args, wanted).await
    }

    async fn list_recent_files(&self, args: &Args) -> Result<Value, String> {
        let order = match args.word("orderBy") {
            Some("lastModified") => "modifiedTime desc",
            Some("lastModifiedByMe") => "modifiedByMeTime desc",
            _ => "recency desc",
        };
        self.list(
            args,
            vec![
                ("q", "trashed = false".to_owned()),
                ("orderBy", order.to_owned()),
            ],
        )
        .await
    }

    async fn list(&self, args: &Args, mut query: Vec<(&str, String)>) -> Result<Value, String> {
        query.extend([
            (
                "pageSize",
                args.page("pageSize", PAGE, MOST_PAGE).to_string(),
            ),
            ("fields", format!("nextPageToken,files({FIELDS})")),
            ("supportsAllDrives", "true".to_owned()),
            ("includeItemsFromAllDrives", "true".to_owned()),
        ]);
        if let Some(token) = args.word("pageToken") {
            query.push(("pageToken", token.to_owned()));
        }
        let found = self.rest.get(self.url(&["files"], &query)?).await?;
        let rich = args.flag("includeRichMetadata");
        let mut out = json!({
            "files": items(&found, "files").map(|item| file(item, rich)).collect::<Vec<_>>(),
        });
        copy(&found, &mut out, "nextPageToken");
        Ok(out)
    }

    async fn get_file_metadata(&self, args: &Args) -> Result<Value, String> {
        let found = self.metadata(&args.text("fileId")?, FIELDS).await?;
        Ok(file(&found, args.flag("includeRichMetadata")))
    }

    async fn get_file_permissions(&self, args: &Args) -> Result<Value, String> {
        let id = args.text("fileId")?;
        let found = self
            .rest
            .get(self.url(
                &["files", &id, "permissions"],
                &[
                    (
                        "fields",
                        "permissions(id,type,role,emailAddress,displayName,expirationTime,pendingOwner,view)"
                            .to_owned(),
                    ),
                    ("supportsAllDrives", "true".to_owned()),
                ],
            )?)
            .await?;
        Ok(json!({ "permissions": found["permissions"] }))
    }

    async fn read_file_content(&self, args: &Args) -> Result<Value, String> {
        let id = args.text("fileId")?;
        let found = self
            .metadata(&id, "id,name,mimeType,webViewLink,driveId")
            .await?;
        let mime = found["mimeType"].as_str().unwrap_or_default();
        let content = match reading(mime) {
            Some(Reading::Export(target)) => self.export(&id, target).await?,
            Some(Reading::Media) => self.media(&id, None).await?,
            Some(Reading::Convert(target)) => self.converted(&id, target).await?,
            None => {
                return Err(format!(
                    "{mime} files cannot be read as text; download_file_content returns their bytes"
                ));
            }
        };
        let mut out = json!({
            "title": found["name"],
            "viewUrl": found["webViewLink"],
            "fileContent": String::from_utf8_lossy(&content.bytes),
        });
        rename(&found, "driveId", &mut out, "sharedDriveId");
        if args.flag("includeComments") {
            if mime.starts_with(NATIVE) {
                out["commentThreads"] = Value::Array(comments(&self.rest, &self.base, &id).await?);
            } else {
                out["commentsNotSupported"] = Value::Bool(true);
            }
        }
        Ok(out)
    }

    async fn download_file_content(&self, args: &Args) -> Result<Value, String> {
        let id = args.text("fileId")?;
        let found = self.metadata(&id, "id,name,mimeType").await?;
        let mime = found["mimeType"].as_str().unwrap_or_default();
        let (content, sent_as) = if mime.starts_with(NATIVE) {
            let target = match args.word("exportMimeType") {
                Some(wanted) => wanted,
                None => plain_export(mime)
                    .ok_or_else(|| format!("{mime} files cannot be downloaded"))?,
            };
            (self.export(&id, target).await?, target.to_owned())
        } else {
            (
                self.media(&id, args.word("revisionId")).await?,
                mime.to_owned(),
            )
        };
        if content.is_cut {
            return Err(format!(
                "{} is larger than {} MB, the most goat downloads",
                found["name"].as_str().unwrap_or(&id),
                DOWNLOAD_BYTES / 1024 / 1024
            ));
        }
        Ok(json!({
            "fileId": id,
            "title": found["name"],
            "mimeType": sent_as,
            "base64Content": STANDARD.encode(&content.bytes),
        }))
    }

    async fn create_file(&self, args: &Args) -> Result<Value, String> {
        let mut metadata = json!({ "name": args.text("title")? });
        if let Some(parent) = args.word("parentId") {
            metadata["parents"] = json!([parent]);
        }
        if let Some(id) = args.word("pregeneratedId") {
            metadata["id"] = Value::String(id.to_owned());
        }
        if args.has("inheritedPermissionsDisabled") {
            metadata["inheritedPermissionsDisabled"] =
                Value::Bool(args.flag("inheritedPermissionsDisabled"));
        }
        let mime = args
            .word("contentMimeType")
            .or_else(|| args.word("mimeType"));
        let fields = [
            ("fields", FIELDS.to_owned()),
            ("supportsAllDrives", "true".to_owned()),
        ];
        let Some(bytes) = content(args, &["base64Content", "content"])? else {
            if let Some(mime) = mime {
                metadata["mimeType"] = Value::String(mime.to_owned());
            }
            let created = self
                .rest
                .send(Method::POST, self.url(&["files"], &fields)?, &metadata)
                .await?;
            return Ok(file(&created, false));
        };
        let mime = mime.ok_or("contentMimeType is required when content is given")?;
        if !args.flag("disableConversionToGoogleType")
            && let Some(target) = import_target(mime)
        {
            metadata["mimeType"] = Value::String(target.to_owned());
        }
        let boundary = format!("goat-{}", oauth::random_token::<16>());
        let body = multipart(&boundary, &metadata, mime, &bytes);
        let mut query = vec![("uploadType", "multipart".to_owned())];
        query.extend(fields);
        let created = self
            .rest
            .upload(
                Method::POST,
                self.upload_url(&["files"], &query)?,
                &format!("multipart/related; boundary={boundary}"),
                body,
            )
            .await?;
        Ok(file(&created, false))
    }

    async fn copy_file(&self, args: &Args) -> Result<Value, String> {
        let id = args.text("fileId")?;
        let mut body = json!({});
        if let Some(title) = args.word("title") {
            body["name"] = Value::String(title.to_owned());
        }
        if let Some(parent) = args.word("parentId") {
            body["parents"] = json!([parent]);
        }
        if let Some(pregenerated) = args.word("pregeneratedId") {
            body["id"] = Value::String(pregenerated.to_owned());
        }
        let copied = self
            .rest
            .send(
                Method::POST,
                self.url(
                    &["files", &id, "copy"],
                    &[
                        ("fields", FIELDS.to_owned()),
                        ("supportsAllDrives", "true".to_owned()),
                    ],
                )?,
                &body,
            )
            .await?;
        Ok(file(&copied, false))
    }

    async fn upload_file_revision(&self, args: &Args) -> Result<Value, String> {
        let id = args.text("fileId")?;
        if args.word("etag").is_some() {
            return Err("goat's Drive server cannot check an etag; leave it out".to_owned());
        }
        let mime = args.text("contentMimeType")?;
        let bytes =
            content(args, &["base64Content"])?.ok_or("textContent or base64Content is required")?;
        let uploaded = self
            .rest
            .upload(
                Method::PATCH,
                self.upload_url(
                    &["files", &id],
                    &[
                        ("uploadType", "media".to_owned()),
                        ("fields", "id,headRevisionId".to_owned()),
                        ("supportsAllDrives", "true".to_owned()),
                    ],
                )?,
                &mime,
                bytes,
            )
            .await?;
        let revision = uploaded["headRevisionId"]
            .as_str()
            .ok_or("Google Drive did not name the new revision")?;
        let found = self
            .rest
            .get(self.url(
                &["files", &id, "revisions", revision],
                &[(
                    "fields",
                    "id,mimeType,modifiedTime,keepForever,lastModifyingUser(displayName),publishAuto,publishedOutsideDomain"
                        .to_owned(),
                )],
            )?)
            .await?;
        let mut out = json!({ "id": found["id"], "canDownload": true });
        copy(&found, &mut out, "mimeType");
        copy(&found, &mut out, "modifiedTime");
        rename(&found, "keepForever", &mut out, "pinned");
        rename(
            &found["lastModifyingUser"],
            "displayName",
            &mut out,
            "lastModifyingUserName",
        );
        Ok(out)
    }

    async fn metadata(&self, id: &str, fields: &str) -> Result<Value, String> {
        self.rest
            .get(self.url(
                &["files", id],
                &[
                    ("fields", fields.to_owned()),
                    ("supportsAllDrives", "true".to_owned()),
                ],
            )?)
            .await
    }

    async fn export(&self, id: &str, mime: &str) -> Result<google::Download, String> {
        let url = self.url(&["files", id, "export"], &[("mimeType", mime.to_owned())])?;
        self.rest.download(url, DOWNLOAD_BYTES).await
    }

    async fn media(&self, id: &str, revision: Option<&str>) -> Result<google::Download, String> {
        let query = [
            ("alt", "media".to_owned()),
            ("supportsAllDrives", "true".to_owned()),
        ];
        let url = match revision {
            Some(revision) => self.url(&["files", id, "revisions", revision], &query)?,
            None => self.url(&["files", id], &query)?,
        };
        self.rest.download(url, DOWNLOAD_BYTES).await
    }

    async fn converted(&self, id: &str, target: &str) -> Result<google::Download, String> {
        let copied = self
            .rest
            .send(
                Method::POST,
                self.url(
                    &["files", id, "copy"],
                    &[
                        ("fields", "id".to_owned()),
                        ("supportsAllDrives", "true".to_owned()),
                    ],
                )?,
                &json!({ "name": format!("goat reading {id}"), "mimeType": target, "parents": ["root"] }),
            )
            .await?;
        let copy_id = copied["id"]
            .as_str()
            .ok_or("Google Drive did not return the converted copy")?;
        let read = match plain_export(target) {
            Some(export) => self.export(copy_id, export).await,
            None => Err(format!("{target} has no text export")),
        };
        let removed = self.rest.delete(self.url(&["files", copy_id], &[])?).await;
        let content = read?;
        removed.map_err(|problem| {
            format!("the converted copy {copy_id} in My Drive could not be deleted: {problem}")
        })?;
        Ok(content)
    }

    fn url(&self, segments: &[&str], query: &[(&str, String)]) -> Result<Url, String> {
        drive_url(&self.base, segments, query)
    }

    fn upload_url(&self, segments: &[&str], query: &[(&str, String)]) -> Result<Url, String> {
        let path: Vec<&str> = ["upload", "drive", "v3"]
            .into_iter()
            .chain(segments.iter().copied())
            .collect();
        google::at(&self.base, &path, query)
    }
}

pub(super) async fn comments(rest: &Rest, base: &str, id: &str) -> Result<Vec<Value>, String> {
    let found = rest
        .get(drive_url(
            base,
            &["files", id, "comments"],
            &[
                ("fields", COMMENT_FIELDS.to_owned()),
                ("pageSize", COMMENTS.to_owned()),
            ],
        )?)
        .await?;
    Ok(items(&found, "comments")
        .map(|comment| {
            json!({
                "commentId": comment["id"],
                "status": if comment["resolved"].as_bool() == Some(true) { "RESOLVED" } else { "OPEN" },
                "headPost": post(comment),
                "replies": items(comment, "replies").map(post).collect::<Vec<_>>(),
            })
        })
        .collect())
}

fn drive_url(base: &str, segments: &[&str], query: &[(&str, String)]) -> Result<Url, String> {
    let path: Vec<&str> = ["drive", "v3"]
        .into_iter()
        .chain(segments.iter().copied())
        .collect();
    google::at(base, &path, query)
}

fn post(found: &Value) -> Value {
    json!({
        "postId": found["id"],
        "authorName": found["author"]["displayName"],
        "content": found["content"],
        "modifiedTime": found["modifiedTime"],
    })
}

fn file(found: &Value, rich: bool) -> Value {
    let mut out = json!({});
    for field in [
        "id",
        "mimeType",
        "description",
        "createdTime",
        "modifiedTime",
        "viewedByMeTime",
        "sharedWithMeTime",
        "starred",
        "trashed",
        "fileExtension",
        "writersCanShare",
        "inheritedPermissionsDisabled",
    ] {
        copy(found, &mut out, field);
    }
    rename(found, "name", &mut out, "title");
    rename(found, "size", &mut out, "fileSize");
    rename(found, "webViewLink", &mut out, "viewUrl");
    rename(found, "driveId", &mut out, "sharedDriveId");
    if let Some(parent) = found["parents"][0].as_str() {
        out["parentId"] = Value::String(parent.to_owned());
    }
    let owner = &found["owners"][0];
    rename(owner, "emailAddress", &mut out, "owner");
    if let Some(capabilities) = found.get("capabilities").filter(|value| value.is_object()) {
        out["capabilities"] = capabilities.clone();
        rename(capabilities, "canAddChildren", &mut out, "canAddChildren");
    }
    let restriction = &found["contentRestrictions"][0];
    rename(restriction, "readOnly", &mut out, "readOnly");
    rename(restriction, "reason", &mut out, "readOnlyReason");
    if let Some(shortcut) = found
        .get("shortcutDetails")
        .filter(|value| value.is_object())
    {
        out["shortcutDetails"] = json!({
            "targetFileId": shortcut["targetId"],
            "targetMimeType": shortcut["targetMimeType"],
        });
    }
    if rich {
        rename(owner, "displayName", &mut out, "ownerName");
        let editor = &found["lastModifyingUser"];
        rename(editor, "displayName", &mut out, "lastModifyingUserName");
        rename(editor, "emailAddress", &mut out, "lastModifyingUserEmail");
    }
    out
}

fn translate(query: &str) -> Result<String, String> {
    let tokens = tokens(query);
    let mut out: Vec<String> = Vec::with_capacity(tokens.len());
    let mut at = 0;
    while at < tokens.len() {
        let token = tokens[at].as_str();
        if UNSUPPORTED_TERMS.contains(&token) {
            return Err(format!(
                "goat's Drive server cannot search by {token}; search with other terms"
            ));
        }
        let field = match token {
            "parentId" => Some("parents"),
            "owner" => Some("owners"),
            _ => None,
        };
        if let Some(field) = field {
            let (Some(operator), Some(value)) = (tokens.get(at + 1), tokens.get(at + 2)) else {
                return Err(format!("{token} needs an operator and a value"));
            };
            match operator.as_str() {
                "=" => out.push(format!("{value} in {field}")),
                "!=" => out.push(format!("not {value} in {field}")),
                other => return Err(format!("{token} takes = or !=, not {other}")),
            }
            at += 3;
            continue;
        }
        out.push(if token == "title" { "name" } else { token }.to_owned());
        at += 1;
    }
    Ok(out.join(" "))
}

fn tokens(query: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut chars = query.chars().peekable();
    while let Some(&next) = chars.peek() {
        if next.is_whitespace() {
            chars.next();
        } else if next == '\'' {
            let mut text = String::from(chars.next().unwrap_or(next));
            while let Some(char) = chars.next() {
                text.push(char);
                if char == '\\' {
                    if let Some(escaped) = chars.next() {
                        text.push(escaped);
                    }
                } else if char == '\'' {
                    break;
                }
            }
            found.push(text);
        } else if next == '(' || next == ')' {
            found.push(chars.next().unwrap_or(next).to_string());
        } else if "<>=!".contains(next) {
            let mut operator = String::new();
            while let Some(&char) = chars.peek()
                && "<>=!".contains(char)
            {
                operator.push(char);
                chars.next();
            }
            found.push(operator);
        } else {
            let mut word = String::new();
            while let Some(&char) = chars.peek()
                && !char.is_whitespace()
                && !"()'<>=!".contains(char)
            {
                word.push(char);
                chars.next();
            }
            found.push(word);
        }
    }
    found
}

fn search_order(raw: &str) -> Result<String, String> {
    let mut words = raw.split_whitespace();
    let field = words.next().unwrap_or_default();
    let direction = words.next().unwrap_or("desc");
    if !matches!(field, "modifiedTime" | "viewedByMeTime") || !matches!(direction, "asc" | "desc") {
        return Err(format!(
            "orderBy {raw} is not supported; use modifiedTime or viewedByMeTime, with asc or desc"
        ));
    }
    Ok(format!("{field} {direction}"))
}

fn reading(mime: &str) -> Option<Reading> {
    if mime.starts_with(NATIVE) {
        return plain_export(mime)
            .filter(|_| mime != DRAWING)
            .map(Reading::Export);
    }
    if mime.starts_with("text/")
        || matches!(
            mime,
            "application/json" | "application/xml" | "application/javascript"
        )
    {
        return Some(Reading::Media);
    }
    let target = match mime {
        "application/pdf"
        | "image/png"
        | "image/jpeg"
        | "image/jpg"
        | "application/msword"
        | "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
        | "application/vnd.oasis.opendocument.text"
        | "application/x-vnd.oasis.opendocument.text" => DOCUMENT,
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
        | "application/vnd.oasis.opendocument.spreadsheet" => SPREADSHEET,
        "application/vnd.openxmlformats-officedocument.presentationml.presentation"
        | "application/vnd.oasis.opendocument.presentation" => PRESENTATION,
        _ => return None,
    };
    Some(Reading::Convert(target))
}

fn plain_export(mime: &str) -> Option<&'static str> {
    match mime {
        DOCUMENT => Some("text/markdown"),
        SPREADSHEET => Some("text/csv"),
        PRESENTATION => Some("text/plain"),
        DRAWING => Some("image/png"),
        _ => None,
    }
}

fn import_target(mime: &str) -> Option<&'static str> {
    match mime {
        "text/plain"
        | "text/html"
        | "text/markdown"
        | "application/rtf"
        | "application/msword"
        | "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
        | "application/vnd.oasis.opendocument.text" => Some(DOCUMENT),
        "text/csv"
        | "text/tab-separated-values"
        | "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
        | "application/vnd.oasis.opendocument.spreadsheet" => Some(SPREADSHEET),
        "application/vnd.openxmlformats-officedocument.presentationml.presentation"
        | "application/vnd.oasis.opendocument.presentation" => Some(PRESENTATION),
        _ => None,
    }
}

fn content(args: &Args, base64_fields: &[&str]) -> Result<Option<Vec<u8>>, String> {
    let text = args.word("textContent");
    let encoded = base64_fields.iter().find_map(|field| args.word(field));
    match (text, encoded) {
        (Some(_), Some(_)) => Err("set textContent or base64Content, not both".to_owned()),
        (Some(text), None) => Ok(Some(text.as_bytes().to_vec())),
        (None, Some(encoded)) => STANDARD
            .decode(encoded.trim())
            .map(Some)
            .map_err(|_| "base64Content is not base64".to_owned()),
        (None, None) => Ok(None),
    }
}

fn multipart(boundary: &str, metadata: &Value, mime: &str, bytes: &[u8]) -> Vec<u8> {
    let mut body = Vec::with_capacity(bytes.len() + 512);
    body.extend_from_slice(
        format!(
            "--{boundary}\r\ncontent-type: application/json; charset=UTF-8\r\n\r\n{metadata}\r\n--{boundary}\r\ncontent-type: {mime}\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(bytes);
    body.extend_from_slice(format!("\r\n--{boundary}--").as_bytes());
    body
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::super::google::Product;
    use super::super::google::fake::{self, serve};
    use super::*;

    const FOLDER: &str = "application/vnd.google-apps.folder";

    #[test]
    fn a_query_in_the_official_syntax_becomes_a_drive_query() {
        let query = translate(
            "title contains 'plan' and parentId = 'f1' and owner != 'me' and fullText contains 'it\\'s'",
        )
        .unwrap();

        assert_eq!(
            query,
            "name contains 'plan' and 'f1' in parents and not 'me' in owners and fullText contains 'it\\'s'"
        );
    }

    #[test]
    fn a_term_drive_cannot_search_by_is_refused() {
        let problem = translate("'f1' in ancestors").unwrap_err();

        assert!(problem.contains("ancestors"), "{problem}");
    }

    #[test]
    fn a_quoted_term_name_is_left_alone() {
        assert_eq!(
            translate("fullText contains 'title owner'").unwrap(),
            "fullText contains 'title owner'"
        );
    }

    #[tokio::test]
    async fn a_search_leaves_out_the_trash_and_shapes_files_like_the_official_server() {
        let google = serve(vec![(
            "GET /drive/v3/files",
            fake::json(&json!({
                "files": [{
                    "id": "f1",
                    "name": "Plan",
                    "mimeType": DOCUMENT,
                    "size": "12",
                    "parents": ["root1"],
                    "owners": [{ "emailAddress": "kim@example.com", "displayName": "Kim" }],
                    "webViewLink": "https://docs.google.com/document/d/f1",
                    "contentRestrictions": [{ "readOnly": true, "reason": "final" }],
                }],
                "nextPageToken": "n2",
            })),
        )])
        .await;

        let found = google
            .call(
                Product::Drive,
                "search_files",
                json!({ "query": "title contains 'Plan'", "orderBy": "modifiedTime" }),
            )
            .await
            .unwrap();

        let seen = google.one("GET", "/drive/v3/files");
        assert_eq!(
            seen.query("q"),
            ["(name contains 'Plan') and trashed = false"]
        );
        assert_eq!(seen.query("orderBy"), ["modifiedTime desc"]);
        assert_eq!(
            found,
            json!({
                "files": [{
                    "id": "f1",
                    "title": "Plan",
                    "mimeType": DOCUMENT,
                    "fileSize": "12",
                    "parentId": "root1",
                    "owner": "kim@example.com",
                    "viewUrl": "https://docs.google.com/document/d/f1",
                    "readOnly": true,
                    "readOnlyReason": "final",
                }],
                "nextPageToken": "n2",
            })
        );
    }

    #[tokio::test]
    async fn a_google_doc_is_read_as_markdown_with_its_comments() {
        let google = serve(vec![
            (
                "GET /drive/v3/files/d1/export",
                fake::bytes("text/markdown", "# 회의\n".as_bytes()),
            ),
            (
                "GET /drive/v3/files/d1/comments",
                fake::json(&json!({ "comments": [{
                    "id": "c1", "content": "Why?", "resolved": false,
                    "author": { "displayName": "Kim" },
                    "replies": [{ "id": "r1", "content": "Because", "author": { "displayName": "Lee" } }],
                }]})),
            ),
            (
                "GET /drive/v3/files/d1",
                fake::json(&json!({ "id": "d1", "name": "Notes", "mimeType": DOCUMENT })),
            ),
        ])
        .await;

        let read = google
            .call(
                Product::Drive,
                "read_file_content",
                json!({ "fileId": "d1", "includeComments": true }),
            )
            .await
            .unwrap();

        assert_eq!(
            google
                .one("GET", "/drive/v3/files/d1/export")
                .query("mimeType"),
            ["text/markdown"]
        );
        assert_eq!(read["fileContent"], "# 회의\n");
        assert_eq!(read["commentThreads"][0]["status"], "OPEN");
        assert_eq!(read["commentThreads"][0]["replies"][0]["authorName"], "Lee");
    }

    #[tokio::test]
    async fn a_pdf_is_read_through_a_converted_copy_that_is_then_deleted() {
        let google = serve(vec![
            (
                "POST /drive/v3/files/p1/copy",
                fake::json(&json!({ "id": "tmp" })),
            ),
            (
                "GET /drive/v3/files/tmp/export",
                fake::bytes("text/markdown", b"scanned text"),
            ),
            ("DELETE /drive/v3/files/tmp", fake::status(204, &json!({}))),
            (
                "GET /drive/v3/files/p1",
                fake::json(
                    &json!({ "id": "p1", "name": "scan.pdf", "mimeType": "application/pdf" }),
                ),
            ),
        ])
        .await;

        let read = google
            .call(
                Product::Drive,
                "read_file_content",
                json!({ "fileId": "p1" }),
            )
            .await
            .unwrap();

        assert_eq!(read["fileContent"], "scanned text");
        assert_eq!(
            google.one("POST", "/drive/v3/files/p1/copy").json()["mimeType"],
            DOCUMENT
        );
        google.one("DELETE", "/drive/v3/files/tmp");
    }

    #[tokio::test]
    async fn the_converted_copy_is_deleted_even_when_reading_it_fails() {
        let google = serve(vec![
            (
                "POST /drive/v3/files/p1/copy",
                fake::json(&json!({ "id": "tmp" })),
            ),
            (
                "GET /drive/v3/files/tmp/export",
                fake::status(500, &json!({ "error": { "message": "boom" } })),
            ),
            ("DELETE /drive/v3/files/tmp", fake::status(204, &json!({}))),
            (
                "GET /drive/v3/files/p1",
                fake::json(
                    &json!({ "id": "p1", "name": "scan.pdf", "mimeType": "application/pdf" }),
                ),
            ),
        ])
        .await;

        let problem = google
            .call(
                Product::Drive,
                "read_file_content",
                json!({ "fileId": "p1" }),
            )
            .await
            .unwrap_err();

        assert_eq!(problem, "Google Drive returned 500: boom");
        google.one("DELETE", "/drive/v3/files/tmp");
    }

    #[tokio::test]
    async fn a_text_upload_becomes_a_google_doc_unless_conversion_is_off() {
        let google = serve(vec![(
            "POST /upload/drive/v3/files",
            fake::json(&json!({ "id": "n1", "name": "Notes" })),
        )])
        .await;

        google
            .call(
                Product::Drive,
                "create_file",
                json!({ "title": "Notes", "textContent": "안녕", "contentMimeType": "text/plain", "parentId": "f1" }),
            )
            .await
            .unwrap();
        google
            .call(
                Product::Drive,
                "create_file",
                json!({ "title": "Notes", "textContent": "안녕", "contentMimeType": "text/plain", "disableConversionToGoogleType": true }),
            )
            .await
            .unwrap();

        let sent = google.seen("POST", "/upload/drive/v3/files");
        assert_eq!(sent[0].query("uploadType"), ["multipart"]);
        assert!(
            sent[0]
                .text()
                .contains(&format!("\"mimeType\":\"{DOCUMENT}\""))
        );
        assert!(sent[0].text().contains("\"parents\":[\"f1\"]"));
        assert!(sent[0].text().contains("안녕"));
        assert!(!sent[1].text().contains(DOCUMENT));
    }

    #[tokio::test]
    async fn a_folder_is_created_without_content() {
        let google = serve(vec![(
            "POST /drive/v3/files",
            fake::json(&json!({ "id": "d1", "name": "Reports", "mimeType": FOLDER })),
        )])
        .await;

        let created = google
            .call(
                Product::Drive,
                "create_file",
                json!({ "title": "Reports", "contentMimeType": FOLDER }),
            )
            .await
            .unwrap();

        assert_eq!(
            google.one("POST", "/drive/v3/files").json(),
            json!({ "name": "Reports", "mimeType": FOLDER })
        );
        assert_eq!(created["title"], "Reports");
    }

    #[tokio::test]
    async fn a_download_over_the_limit_is_refused() {
        let big = vec![b'a'; DOWNLOAD_BYTES + 1];
        let google = serve(vec![(
            "GET /drive/v3/files/b1",
            fake::bytes("application/octet-stream", &big),
        )])
        .await;

        let problem = google
            .call(
                Product::Drive,
                "download_file_content",
                json!({ "fileId": "b1" }),
            )
            .await
            .unwrap_err();

        assert!(problem.contains("larger than 10 MB"), "{problem}");
    }

    #[tokio::test]
    async fn a_download_is_base64() {
        let google = serve(vec![(
            "GET /drive/v3/files/b1",
            fake::json(
                &json!({ "id": "b1", "name": "a.bin", "mimeType": "application/octet-stream" }),
            ),
        )])
        .await;

        let downloaded = google
            .call(
                Product::Drive,
                "download_file_content",
                json!({ "fileId": "b1" }),
            )
            .await
            .unwrap();

        assert_eq!(downloaded["mimeType"], "application/octet-stream");
        assert_eq!(
            STANDARD
                .decode(downloaded["base64Content"].as_str().unwrap())
                .unwrap(),
            json!({ "id": "b1", "name": "a.bin", "mimeType": "application/octet-stream" })
                .to_string()
                .into_bytes()
        );
    }
}
