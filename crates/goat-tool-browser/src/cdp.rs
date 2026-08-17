use std::sync::Arc;

use chromiumoxide_cdp::cdp::browser_protocol::input::{
    DispatchKeyEventParams, DispatchKeyEventType, DispatchMouseEventParams, DispatchMouseEventType,
    InsertTextParams, MouseButton,
};
use chromiumoxide_cdp::cdp::browser_protocol::network::EnableParams as NetworkEnableParams;
use chromiumoxide_cdp::cdp::browser_protocol::page::{
    CaptureScreenshotFormat, CaptureScreenshotParams, EnableParams as PageEnableParams,
    NavigateParams, StopLoadingParams,
};
use chromiumoxide_cdp::cdp::js_protocol::runtime::{
    CallFunctionOnParams, EnableParams as RuntimeEnableParams, EvaluateParams, ExceptionDetails,
    RemoteObject, RemoteObjectId,
};
use chromiumoxide_types::{Command, MethodType};
use goat_api::{BrowserCommand, BrowserTab, CdpEvent, HostBrowserOutput};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use tokio::sync::broadcast;

use crate::error::BrowserError;
use crate::keys::KeyStroke;
use crate::transport::Transport;

const SCREENSHOT_QUALITY: i64 = 60;

#[derive(Clone)]
pub struct Cdp {
    transport: Arc<dyn Transport>,
}

impl Cdp {
    pub fn new(transport: Arc<dyn Transport>) -> Self {
        Self { transport }
    }

    pub fn events(&self) -> broadcast::Receiver<CdpEvent> {
        self.transport.events()
    }

    pub async fn send<C: Command>(&self, params: C) -> Result<C::Response, BrowserError> {
        let method = params.identifier().into_owned();
        let payload = serde_json::to_value(&params)
            .map_err(|err| BrowserError::Message(format!("{method}: {err}")))?;
        let result = self.raw(method.clone(), payload).await?;
        C::response_from_value(result)
            .map_err(|err| BrowserError::Message(format!("{method} answered unexpectedly: {err}")))
    }

    pub async fn raw(&self, method: String, params: Value) -> Result<Value, BrowserError> {
        match self
            .transport
            .call(BrowserCommand::Cdp { method, params })
            .await?
        {
            HostBrowserOutput::Cdp { result } if result.is_null() => {
                Ok(Value::Object(serde_json::Map::new()))
            }
            HostBrowserOutput::Cdp { result } => Ok(result),
            other => Err(unexpected(&other)),
        }
    }

    pub async fn attach(&self) -> Result<(), BrowserError> {
        self.send(PageEnableParams::default()).await?;
        self.send(RuntimeEnableParams::default()).await?;
        self.send(NetworkEnableParams::default()).await?;
        Ok(())
    }

    pub async fn eval(&self, expression: &str) -> Result<Option<Value>, BrowserError> {
        let params = EvaluateParams::builder()
            .expression(expression)
            .return_by_value(true)
            .await_promise(true)
            .build()
            .map_err(BrowserError::Message)?;
        let returns = self.send(params).await?;
        settled(returns.exception_details, returns.result)
    }

    pub async fn find_element(&self, selector: &str) -> Result<Element, BrowserError> {
        let literal = serde_json::to_string(selector)
            .map_err(|err| BrowserError::Message(err.to_string()))?;
        let params = EvaluateParams::builder()
            .expression(format!("document.querySelector({literal})"))
            .build()
            .map_err(BrowserError::Message)?;
        let returns = self.send(params).await?;
        if let Some(details) = returns.exception_details {
            return Err(thrown(&details));
        }
        let object_id = returns
            .result
            .object_id
            .ok_or_else(|| BrowserError::Input(format!("no element matches {selector}")))?;
        Ok(Element {
            cdp: self.clone(),
            object_id,
        })
    }

    pub async fn url(&self) -> Result<String, BrowserError> {
        Ok(self
            .eval("location.href")
            .await?
            .as_ref()
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned())
    }

    pub async fn navigate(&self, url: String) -> Result<(), BrowserError> {
        self.send(NavigateParams::new(url)).await?;
        Ok(())
    }

    pub async fn stop_loading(&self) -> Result<(), BrowserError> {
        self.send(StopLoadingParams::default()).await?;
        Ok(())
    }

    pub async fn mouse(
        &self,
        kind: DispatchMouseEventType,
        x: f64,
        y: f64,
        with_button: bool,
    ) -> Result<(), BrowserError> {
        let mut builder = DispatchMouseEventParams::builder().r#type(kind).x(x).y(y);
        if with_button {
            builder = builder.button(MouseButton::Left).click_count(1);
        }
        let params = builder.build().map_err(BrowserError::Message)?;
        self.send(params).await?;
        Ok(())
    }

    pub async fn key(&self, stroke: &KeyStroke) -> Result<(), BrowserError> {
        let mut down = DispatchKeyEventParams::builder()
            .r#type(DispatchKeyEventType::KeyDown)
            .key(stroke.key_name())
            .windows_virtual_key_code(stroke.virtual_key)
            .native_virtual_key_code(stroke.virtual_key);
        let mut up = DispatchKeyEventParams::builder()
            .r#type(DispatchKeyEventType::KeyUp)
            .key(stroke.key_name())
            .windows_virtual_key_code(stroke.virtual_key)
            .native_virtual_key_code(stroke.virtual_key);
        let code = stroke.code_name();
        if !code.is_empty() {
            down = down.code(code.clone());
            up = up.code(code);
        }
        if !stroke.text.is_empty() {
            down = down
                .text(stroke.text.clone())
                .unmodified_text(stroke.text.clone());
        }
        self.send(down.build().map_err(BrowserError::Message)?)
            .await?;
        self.send(up.build().map_err(BrowserError::Message)?)
            .await?;
        Ok(())
    }

    pub async fn insert_text(&self, text: &str) -> Result<(), BrowserError> {
        self.send(InsertTextParams::new(text.to_owned())).await?;
        Ok(())
    }

    pub async fn screenshot(&self) -> Result<String, BrowserError> {
        let params = CaptureScreenshotParams::builder()
            .format(CaptureScreenshotFormat::Jpeg)
            .quality(SCREENSHOT_QUALITY)
            .capture_beyond_viewport(false)
            .build();
        Ok(self.send(params).await?.data.into())
    }

    pub async fn tabs(&self, command: BrowserCommand) -> Result<Vec<BrowserTab>, BrowserError> {
        match self.transport.call(command).await? {
            HostBrowserOutput::Tabs { tabs } => Ok(tabs),
            other => Err(unexpected(&other)),
        }
    }

    pub async fn detach(&self) -> Result<(), BrowserError> {
        match self.transport.call(BrowserCommand::Detach {}).await? {
            HostBrowserOutput::Detached {} => Ok(()),
            other => Err(unexpected(&other)),
        }
    }
}

pub struct Element {
    cdp: Cdp,
    object_id: RemoteObjectId,
}

#[derive(Debug, Clone, Copy, Deserialize)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Rect {
    pub fn center(&self) -> (f64, f64) {
        (self.x + self.width / 2.0, self.y + self.height / 2.0)
    }
}

impl Element {
    pub fn object_id(&self) -> RemoteObjectId {
        self.object_id.clone()
    }

    pub async fn call_js_fn(
        &self,
        declaration: impl Into<String>,
    ) -> Result<Option<Value>, BrowserError> {
        let params = CallFunctionOnParams::builder()
            .function_declaration(declaration)
            .object_id(self.object_id.clone())
            .return_by_value(true)
            .build()
            .map_err(BrowserError::Message)?;
        let returns = self.cdp.send(params).await?;
        settled(returns.exception_details, returns.result)
    }

    pub async fn scroll_into_view(&self) -> Result<(), BrowserError> {
        self.call_js_fn(
            "function() { this.scrollIntoView({ block: 'center', inline: 'center' }); }",
        )
        .await?;
        Ok(())
    }

    pub async fn rect(&self) -> Result<Rect, BrowserError> {
        let value = self
            .call_js_fn(
                "function() { const r = this.getBoundingClientRect(); return { x: r.left, y: r.top, width: r.width, height: r.height }; }",
            )
            .await?
            .ok_or_else(|| BrowserError::Message("element has no box".to_owned()))?;
        serde_json::from_value(value).map_err(|err| BrowserError::Message(err.to_string()))
    }

    pub async fn click(&self) -> Result<(), BrowserError> {
        let (x, y) = self.rect().await?.center();
        self.cdp
            .mouse(DispatchMouseEventType::MouseMoved, x, y, false)
            .await?;
        self.cdp
            .mouse(DispatchMouseEventType::MousePressed, x, y, true)
            .await?;
        self.cdp
            .mouse(DispatchMouseEventType::MouseReleased, x, y, true)
            .await
    }

    pub async fn hover(&self) -> Result<(), BrowserError> {
        let (x, y) = self.rect().await?.center();
        self.cdp
            .mouse(DispatchMouseEventType::MouseMoved, x, y, false)
            .await
    }
}

pub fn decode<E>(event: &CdpEvent) -> Option<E>
where
    E: MethodType + DeserializeOwned,
{
    if event.method.as_str() != E::method_id().as_ref() {
        return None;
    }
    serde_json::from_value(event.params.clone()).ok()
}

pub async fn next_event(events: &mut broadcast::Receiver<CdpEvent>) -> Option<CdpEvent> {
    loop {
        match events.recv().await {
            Ok(event) => return Some(event),
            Err(broadcast::error::RecvError::Lagged(_)) => {}
            Err(broadcast::error::RecvError::Closed) => return None,
        }
    }
}

fn settled(
    exception: Option<ExceptionDetails>,
    result: RemoteObject,
) -> Result<Option<Value>, BrowserError> {
    match exception {
        Some(details) => Err(thrown(&details)),
        None => Ok(result.value),
    }
}

fn thrown(details: &ExceptionDetails) -> BrowserError {
    let text = details
        .exception
        .as_ref()
        .and_then(|object| object.description.clone())
        .unwrap_or_else(|| details.text.clone());
    BrowserError::Message(format!("javascript exception: {text}"))
}

fn unexpected(output: &HostBrowserOutput) -> BrowserError {
    BrowserError::Message(format!("the browser answered with {output:?}"))
}
