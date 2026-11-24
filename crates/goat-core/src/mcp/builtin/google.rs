use std::sync::Arc;

use reqwest::{Method, Url};
use rmcp::ErrorData;
use rmcp::handler::server::ServerHandler;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, Implementation,
    InitializeResult, ListToolsResult, PaginatedRequestParams, ServerCapabilities, Tool,
};
use rmcp::service::{RequestContext, RoleServer};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use super::{calendar, contacts, docs, drive, gmail, sheets, slides};
use crate::mcp::{Context, Error};
use crate::oauth::{self, Listener, Pkce};
use crate::provider::Endpoints;
use crate::store::Logins;

const CLIENT_ID: &str = "37164328822-ok9stl3msms3abtulohl6tm18rf0g0db.apps.googleusercontent.com";
const CLIENT_SECRET: &str = "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";
const REFRESH_MARGIN_SECS: i64 = 300;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Product {
    Gmail,
    Calendar,
    Drive,
    Docs,
    Sheets,
    Slides,
    Contacts,
}

#[derive(Debug)]
pub(crate) struct Server {
    product: Product,
    service: Service,
    tools: Arc<[Tool]>,
}

#[derive(Debug)]
enum Service {
    Gmail(gmail::Gmail),
    Calendar(calendar::Calendar),
    Drive(drive::Drive),
    Docs(docs::Docs),
    Sheets(sheets::Sheets),
    Slides(slides::Slides),
    Contacts(contacts::Contacts),
}

#[derive(Debug)]
pub(super) struct Rest {
    service: &'static str,
    http: reqwest::Client,
    access: Access,
}

pub(super) struct Download {
    pub(super) bytes: Vec<u8>,
    pub(super) is_cut: bool,
}

pub(super) struct Args(pub(super) Map<String, Value>);

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Grant {
    access_token: String,
    refresh_token: String,
    expires_at: i64,
    client_id: String,
}

#[derive(Debug)]
pub(crate) struct Pending {
    listener: Listener,
    pkce: Pkce,
    state: String,
    redirect_uri: String,
    url: String,
}

#[derive(Debug)]
pub(crate) struct Access {
    http: reqwest::Client,
    logins: Arc<Logins>,
    key: String,
    token_url: Box<str>,
}

impl Product {
    const ALL: [Self; 7] = [
        Self::Gmail,
        Self::Calendar,
        Self::Drive,
        Self::Docs,
        Self::Sheets,
        Self::Slides,
        Self::Contacts,
    ];

    pub(crate) fn parse(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|product| product.as_str() == name)
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Gmail => "gmail",
            Self::Calendar => "google_calendar",
            Self::Drive => "google_drive",
            Self::Docs => "google_docs",
            Self::Sheets => "google_sheets",
            Self::Slides => "google_slides",
            Self::Contacts => "google_contacts",
        }
    }

    pub(crate) fn scopes(self) -> &'static [&'static str] {
        match self {
            Self::Gmail => gmail::SCOPES,
            Self::Calendar => calendar::SCOPES,
            Self::Drive => drive::SCOPES,
            Self::Docs => docs::SCOPES,
            Self::Sheets => sheets::SCOPES,
            Self::Slides => slides::SCOPES,
            Self::Contacts => contacts::SCOPES,
        }
    }

    fn service(self) -> &'static str {
        match self {
            Self::Gmail => "Gmail",
            Self::Calendar => "Google Calendar",
            Self::Drive => "Google Drive",
            Self::Docs => "Google Docs",
            Self::Sheets => "Google Sheets",
            Self::Slides => "Google Slides",
            Self::Contacts => "Google People",
        }
    }

    fn tools(self) -> Vec<Tool> {
        let contract = match self {
            Self::Gmail => gmail::TOOLS,
            Self::Calendar => calendar::TOOLS,
            Self::Drive => drive::TOOLS,
            Self::Docs => docs::TOOLS,
            Self::Sheets => sheets::TOOLS,
            Self::Slides => slides::TOOLS,
            Self::Contacts => contacts::TOOLS,
        };
        serde_json::from_str(contract).unwrap_or_else(|err| {
            unreachable!("the {} contract is valid JSON: {err}", self.as_str())
        })
    }
}

impl Server {
    pub(crate) fn new(product: Product, context: &Context, key: String) -> Self {
        let rest = Rest {
            service: product.service(),
            http: context.http.clone(),
            access: Access::new(context, key),
        };
        let at = &context.endpoints;
        let service = match product {
            Product::Gmail => Service::Gmail(gmail::Gmail::new(rest, &at.gmail)),
            Product::Calendar => Service::Calendar(calendar::Calendar::new(rest, &at.calendar)),
            Product::Drive => Service::Drive(drive::Drive::new(rest, &at.drive)),
            Product::Docs => Service::Docs(docs::Docs::new(rest, &at.docs, &at.drive)),
            Product::Sheets => Service::Sheets(sheets::Sheets::new(rest, &at.sheets, &at.drive)),
            Product::Slides => Service::Slides(slides::Slides::new(rest, &at.slides, &at.drive)),
            Product::Contacts => Service::Contacts(contacts::Contacts::new(rest, &at.people)),
        };
        Self {
            product,
            service,
            tools: product.tools().into(),
        }
    }

    pub(super) async fn run(&self, name: &str, args: &Args) -> Result<Value, String> {
        match &self.service {
            Service::Gmail(gmail) => gmail.run(name, args).await,
            Service::Calendar(calendar) => calendar.run(name, args).await,
            Service::Drive(drive) => drive.run(name, args).await,
            Service::Docs(docs) => docs.run(name, args).await,
            Service::Sheets(sheets) => sheets.run(name, args).await,
            Service::Slides(slides) => slides.run(name, args).await,
            Service::Contacts(contacts) => contacts.run(name, args).await,
        }
    }
}

impl Rest {
    pub(super) async fn get(&self, url: Url) -> Result<Value, String> {
        self.call(self.http.get(url)).await
    }

    pub(super) async fn send(
        &self,
        method: Method,
        url: Url,
        body: &Value,
    ) -> Result<Value, String> {
        self.call(self.http.request(method, url).json(body)).await
    }

    pub(super) async fn delete(&self, url: Url) -> Result<Value, String> {
        self.call(self.http.delete(url)).await
    }

    pub(super) async fn upload(
        &self,
        method: Method,
        url: Url,
        content_type: &str,
        bytes: Vec<u8>,
    ) -> Result<Value, String> {
        let request = self
            .http
            .request(method, url)
            .header(reqwest::header::CONTENT_TYPE, content_type)
            .body(bytes);
        self.call(request).await
    }

    pub(super) async fn download(&self, url: Url, limit: usize) -> Result<Download, String> {
        let mut response = self.respond(self.http.get(url)).await?;
        if !response.status().is_success() {
            return Err(self.failure(response).await);
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|err| format!("{}'s answer could not be read: {err}", self.service))?
        {
            let room = limit - bytes.len();
            if chunk.len() > room {
                bytes.extend_from_slice(&chunk[..room]);
                return Ok(Download {
                    bytes,
                    is_cut: true,
                });
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(Download {
            bytes,
            is_cut: false,
        })
    }

    async fn call(&self, request: reqwest::RequestBuilder) -> Result<Value, String> {
        let response = self.respond(request).await?;
        if !response.status().is_success() {
            return Err(self.failure(response).await);
        }
        let text = response
            .text()
            .await
            .map_err(|err| format!("{}'s answer could not be read: {err}", self.service))?;
        if text.trim().is_empty() {
            return Ok(json!({}));
        }
        Ok(serde_json::from_str(&text).unwrap_or(Value::String(text)))
    }

    async fn respond(&self, request: reqwest::RequestBuilder) -> Result<reqwest::Response, String> {
        let bearer = self.access.bearer().await?;
        request
            .bearer_auth(bearer)
            .send()
            .await
            .map_err(|err| format!("{} could not be reached: {err}", self.service))
    }

    async fn failure(&self, response: reqwest::Response) -> String {
        let status = response.status().as_u16();
        let text = response.text().await.unwrap_or_default();
        let value: Value = serde_json::from_str(&text).unwrap_or(Value::String(text));
        let message = value["error"]["message"]
            .as_str()
            .map_or_else(|| value.to_string(), str::to_owned);
        format!("{} returned {status}: {message}", self.service)
    }
}

impl Args {
    pub(super) fn word(&self, key: &str) -> Option<&str> {
        self.0
            .get(key)
            .and_then(Value::as_str)
            .filter(|text| !text.is_empty())
    }

    pub(super) fn text(&self, key: &str) -> Result<String, String> {
        self.word(key)
            .map(str::to_owned)
            .ok_or_else(|| format!("{key} is required"))
    }

    pub(super) fn list(&self, key: &str) -> Vec<String> {
        self.0
            .get(key)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect()
    }

    pub(super) fn flag(&self, key: &str) -> bool {
        self.0.get(key).and_then(Value::as_bool) == Some(true)
    }

    pub(super) fn has(&self, key: &str) -> bool {
        self.0.get(key).is_some_and(|value| !value.is_null())
    }

    pub(super) fn value(&self, key: &str) -> Option<&Value> {
        self.0.get(key).filter(|value| !value.is_null())
    }

    pub(super) fn objects(&self, key: &str) -> impl Iterator<Item = &Value> {
        self.0
            .get(key)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|item| item.is_object())
    }

    pub(super) fn page(&self, key: &str, default: u64, most: u64) -> u64 {
        self.0
            .get(key)
            .and_then(Value::as_u64)
            .filter(|size| *size > 0)
            .unwrap_or(default)
            .min(most)
    }
}

impl ServerHandler for Server {
    fn get_info(&self) -> InitializeResult {
        InitializeResult::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new(
                self.product.as_str(),
                env!("CARGO_PKG_VERSION"),
            ))
    }

    fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ListToolsResult, ErrorData>> + Send + '_ {
        std::future::ready(Ok(ListToolsResult::with_all_items(self.tools.to_vec())))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let args = Args(request.arguments.unwrap_or_default());
        let result = match self.run(&request.name, &args).await {
            Ok(value) => CallToolResult::structured(value),
            Err(problem) => CallToolResult::error(vec![ContentBlock::text(problem)]),
        };
        Ok(result.into())
    }
}

impl Pending {
    pub(crate) async fn start(
        scopes: &[&str],
        endpoints: &Endpoints,
    ) -> Result<Self, oauth::Error> {
        let listener = Listener::bind(&[0]).await?;
        let redirect_uri = listener.redirect_uri()?;
        let pkce = Pkce::generate();
        let state = oauth::random_token::<32>();
        let scope = scopes.join(" ");
        let url = oauth::authorize_url(
            &endpoints.google_sign_in,
            &[
                ("response_type", "code"),
                ("client_id", CLIENT_ID),
                ("redirect_uri", &redirect_uri),
                ("scope", &scope),
                ("state", &state),
                ("code_challenge", &pkce.challenge),
                ("code_challenge_method", "S256"),
                ("access_type", "offline"),
                ("prompt", "consent select_account"),
            ],
        );
        Ok(Self {
            listener,
            pkce,
            state,
            redirect_uri,
            url,
        })
    }

    pub(crate) fn url(&self) -> &str {
        &self.url
    }

    pub(crate) async fn finish(self, context: &Context, key: &str) -> Result<(), Error> {
        let (http, logins, endpoints) = (&context.http, &context.logins, &context.endpoints);
        let callback = self.listener.callback().await?;
        let (code, _) = callback.into_code(&self.state)?;
        let issued = oauth::request_token(
            http,
            &endpoints.google_token,
            &[
                ("grant_type", "authorization_code"),
                ("code", &code),
                ("redirect_uri", &self.redirect_uri),
                ("client_id", CLIENT_ID),
                ("client_secret", CLIENT_SECRET),
                ("code_verifier", &self.pkce.verifier),
            ],
        )
        .await?;
        let refresh_token = issued.refresh_token.ok_or(oauth::Error::InvalidToken(
            "Google returned no refresh token",
        ))?;
        let token = Grant {
            access_token: issued.access_token,
            refresh_token,
            expires_at: oauth::now() + issued.expires_in.unwrap_or(3600),
            client_id: CLIENT_ID.to_owned(),
        };
        save(logins, key, &token)
    }
}

impl Access {
    pub(crate) fn new(context: &Context, key: String) -> Self {
        Self {
            http: context.http.clone(),
            logins: Arc::clone(&context.logins),
            key,
            token_url: context.endpoints.google_token.clone(),
        }
    }

    pub(crate) async fn bearer(&self) -> Result<String, String> {
        let Some(text) = self.logins.get(&self.key).map_err(|err| err.to_string())? else {
            return Err(SIGN_IN.to_owned());
        };
        let mut token: Grant = serde_json::from_str(&text).map_err(|_| SIGN_IN.to_owned())?;
        if token.client_id != CLIENT_ID {
            return Err(SIGN_IN.to_owned());
        }
        if token.expires_at - REFRESH_MARGIN_SECS > oauth::now() {
            return Ok(token.access_token);
        }
        let issued = oauth::request_token(
            &self.http,
            &self.token_url,
            &[
                ("grant_type", "refresh_token"),
                ("refresh_token", &token.refresh_token),
                ("client_id", CLIENT_ID),
                ("client_secret", CLIENT_SECRET),
            ],
        )
        .await
        .map_err(|err| match oauth::refresh_failure(err) {
            oauth::Error::Reauthenticate(_) => SIGN_IN.to_owned(),
            other => format!("could not refresh the Google sign-in: {other}"),
        })?;
        token.access_token = issued.access_token;
        token.expires_at = oauth::now() + issued.expires_in.unwrap_or(3600);
        if let Some(refresh_token) = issued.refresh_token {
            token.refresh_token = refresh_token;
        }
        save(&self.logins, &self.key, &token).map_err(|err| err.to_string())?;
        Ok(token.access_token)
    }
}

pub(crate) const SIGN_IN: &str =
    "the Google sign-in is missing or expired; ask the user to sign in again";

pub(super) fn at(base: &str, segments: &[&str], query: &[(&str, String)]) -> Result<Url, String> {
    let mut url =
        Url::parse(base).map_err(|err| format!("{base} is not a Google API address: {err}"))?;
    url.path_segments_mut()
        .map_err(|()| format!("{base} is not a Google API address"))?
        .pop_if_empty()
        .extend(segments);
    if !query.is_empty() {
        url.query_pairs_mut()
            .extend_pairs(query.iter().map(|(key, value)| (*key, value.as_str())));
    }
    Ok(url)
}

pub(super) fn no_tool(name: &str) -> String {
    format!("there is no tool named {name}")
}

pub(super) fn items<'a>(value: &'a Value, key: &str) -> impl Iterator<Item = &'a Value> {
    value[key].as_array().into_iter().flatten()
}

pub(super) fn copy(from: &Value, to: &mut Value, field: &str) {
    rename(from, field, to, field);
}

pub(super) fn rename(from: &Value, field: &str, to: &mut Value, as_field: &str) {
    if let Some(value) = from.get(field).filter(|value| !value.is_null()) {
        to[as_field] = value.clone();
    }
}

fn save(logins: &Logins, key: &str, token: &Grant) -> Result<(), Error> {
    let text = serde_json::to_string(token).map_err(oauth::Error::from)?;
    logins.save(key, &text)?;
    Ok(())
}

#[cfg(test)]
pub(super) mod fake {
    use std::sync::{Arc, Mutex};

    use reqwest::Url;
    use serde_json::Value;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};

    use super::{Args, CLIENT_ID, Grant, Product, Server, save};
    use crate::mcp::Context;
    use crate::provider::Endpoints;
    use crate::store::Store;

    const KEY: &str = "mcp:builtin:test";

    pub(crate) struct Reply {
        status: u16,
        content_type: &'static str,
        body: Vec<u8>,
    }

    pub(crate) struct Seen {
        pub(crate) method: String,
        pub(crate) url: Url,
        pub(crate) headers: String,
        pub(crate) body: Vec<u8>,
    }

    pub(crate) struct Google {
        context: Context,
        seen: Arc<Mutex<Vec<Seen>>>,
        _store: Store,
    }

    impl Seen {
        pub(crate) fn query(&self, key: &str) -> Vec<String> {
            self.url
                .query_pairs()
                .filter(|(found, _)| found == key)
                .map(|(_, value)| value.into_owned())
                .collect()
        }

        pub(crate) fn json(&self) -> Value {
            serde_json::from_slice(&self.body).unwrap()
        }

        pub(crate) fn text(&self) -> String {
            String::from_utf8_lossy(&self.body).into_owned()
        }
    }

    impl Google {
        pub(crate) fn server(&self, product: Product) -> Server {
            Server::new(product, &self.context, KEY.to_owned())
        }

        pub(crate) async fn call(
            &self,
            product: Product,
            tool: &str,
            args: Value,
        ) -> Result<Value, String> {
            self.server(product).run(tool, &self::args(args)).await
        }

        pub(crate) fn seen(&self, method: &str, path: &str) -> Vec<Seen> {
            let mut seen = self.seen.lock().unwrap();
            let (matching, rest): (Vec<Seen>, Vec<Seen>) = seen
                .drain(..)
                .partition(|request| request.method == method && request.url.path() == path);
            *seen = rest;
            matching
        }

        pub(crate) fn one(&self, method: &str, path: &str) -> Seen {
            let mut found = self.seen(method, path);
            assert_eq!(
                found.len(),
                1,
                "{method} {path} was sent {} times",
                found.len()
            );
            found.remove(0)
        }
    }

    pub(crate) fn args(value: Value) -> Args {
        let Value::Object(map) = value else {
            unreachable!("tool arguments are an object")
        };
        Args(map)
    }

    pub(crate) fn json(body: &Value) -> Reply {
        status(200, body)
    }

    pub(crate) fn status(status: u16, body: &Value) -> Reply {
        Reply {
            status,
            content_type: "application/json",
            body: body.to_string().into_bytes(),
        }
    }

    pub(crate) fn bytes(content_type: &'static str, body: &[u8]) -> Reply {
        Reply {
            status: 200,
            content_type,
            body: body.to_vec(),
        }
    }

    pub(crate) async fn serve(routes: Vec<(&'static str, Reply)>) -> Google {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&seen);
        let base = url.clone();
        tokio::spawn(async move {
            let routes = Arc::new(routes);
            loop {
                let (mut stream, _) = listener.accept().await.unwrap();
                let request = read(&mut stream, &base).await;
                let key = format!("{} {}", request.method, request.url.path());
                let fallback = Reply {
                    status: 404,
                    content_type: "application/json",
                    body: format!("{{\"error\":{{\"message\":\"no route for {key}\"}}}}")
                        .into_bytes(),
                };
                let reply = routes
                    .iter()
                    .find(|(route, _)| key.starts_with(route))
                    .map_or(&fallback, |(_, reply)| reply);
                let body: &[u8] = if reply.status == 204 {
                    &[]
                } else {
                    &reply.body
                };
                let head = format!(
                    "HTTP/1.1 {} X\r\ncontent-type: {}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                    reply.status,
                    reply.content_type,
                    body.len()
                );
                stream.write_all(head.as_bytes()).await.unwrap();
                stream.write_all(body).await.unwrap();
                stream.shutdown().await.ok();
                log.lock().unwrap().push(request);
            }
        });
        let store = Store::open_in_memory().unwrap();
        let logins = Arc::new(store.logins().unwrap());
        let grant = Grant {
            access_token: "token".to_owned(),
            refresh_token: "refresh".to_owned(),
            expires_at: i64::MAX / 2,
            client_id: CLIENT_ID.to_owned(),
        };
        save(&logins, KEY, &grant).unwrap();
        let at: Box<str> = url.into();
        let endpoints = Endpoints {
            gmail: at.clone(),
            calendar: at.clone(),
            drive: at.clone(),
            docs: at.clone(),
            sheets: at.clone(),
            slides: at.clone(),
            people: at,
            ..Endpoints::default()
        };
        Google {
            context: Context {
                http: reqwest::Client::new(),
                logins,
                endpoints,
                home: std::path::PathBuf::new(),
            },
            seen,
            _store: store,
        }
    }

    async fn read(stream: &mut TcpStream, base: &str) -> Seen {
        let mut buf = Vec::new();
        let mut chunk = [0; 8192];
        let header_end = loop {
            let n = stream.read(&mut chunk).await.unwrap();
            buf.extend_from_slice(&chunk[..n]);
            if let Some(at) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                break at + 4;
            }
        };
        let head = String::from_utf8_lossy(&buf[..header_end]).into_owned();
        let length: usize = head
            .lines()
            .find_map(|line| {
                line.to_lowercase()
                    .strip_prefix("content-length:")
                    .map(|value| value.trim().parse().unwrap())
            })
            .unwrap_or(0);
        while buf.len() < header_end + length {
            let n = stream.read(&mut chunk).await.unwrap();
            buf.extend_from_slice(&chunk[..n]);
        }
        let mut words = head.split_whitespace();
        let method = words.next().unwrap().to_owned();
        let target = words.next().unwrap();
        Seen {
            method,
            url: Url::parse(&format!("{base}{target}")).unwrap(),
            headers: head.clone(),
            body: buf[header_end..].to_vec(),
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::fake::{self, serve};
    use super::*;

    #[test]
    fn every_contract_is_the_official_servers_tool_list() {
        let counts: Vec<(&str, usize)> = Product::ALL
            .into_iter()
            .map(|product| (product.as_str(), product.tools().len()))
            .collect();

        assert_eq!(
            counts,
            [
                ("gmail", 23),
                ("google_calendar", 9),
                ("google_drive", 9),
                ("google_docs", 2),
                ("google_sheets", 6),
                ("google_slides", 4),
                ("google_contacts", 3),
            ]
        );
    }

    #[test]
    fn a_product_is_found_by_its_name() {
        for product in Product::ALL {
            assert_eq!(Product::parse(product.as_str()), Some(product));
        }
        assert_eq!(Product::parse("calendar"), None);
    }

    #[tokio::test]
    async fn every_tool_in_a_contract_is_handled() {
        let google = serve(vec![("", fake::json(&json!({})))]).await;

        for product in Product::ALL {
            for tool in product.tools() {
                let outcome = google.call(product, &tool.name, json!({})).await;

                assert!(
                    !matches!(&outcome, Err(problem) if problem == &no_tool(&tool.name)),
                    "{} {}",
                    product.as_str(),
                    tool.name
                );
            }
        }
    }

    #[tokio::test]
    async fn a_failed_request_is_returned_with_googles_message() {
        let google = serve(vec![(
            "GET",
            fake::status(404, &json!({ "error": { "message": "Not Found" } })),
        )])
        .await;

        let outcome = google
            .call(Product::Calendar, "get_event", json!({ "eventId": "e1" }))
            .await;

        assert_eq!(
            outcome,
            Err("Google Calendar returned 404: Not Found".to_owned())
        );
    }

    #[tokio::test]
    async fn a_request_carries_the_saved_token() {
        let google = serve(vec![("", fake::json(&json!({})))]).await;

        google
            .call(Product::Contacts, "get_user_profile", json!({}))
            .await
            .unwrap();

        let seen = google.one("GET", "/v1/people/me");
        assert!(
            seen.headers
                .to_lowercase()
                .contains("authorization: bearer token")
        );
    }

    #[test]
    fn path_segments_are_escaped() {
        let url = at(
            "https://www.googleapis.com",
            &["calendar", "v3", "calendars", "ko#holiday@group"],
            &[("q", "a b".to_owned())],
        )
        .unwrap();

        assert_eq!(
            url.as_str(),
            "https://www.googleapis.com/calendar/v3/calendars/ko%23holiday@group?q=a+b"
        );
    }
}
