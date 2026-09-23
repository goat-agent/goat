use std::future::Future;

use anyhow::{Error, Result, anyhow};

pub use goat_console::{
    Cell, ColorMode, Footer, Palette, Reported, Settled, Table, blank, confirm, dim,
    format_failure, line, note, pair, pair_styled, pick, prompt, secret, section, success,
    truncate_to_width, warning,
};

pub fn cell<F>(title: &str, body: F) -> Result<Settled>
where
    F: FnOnce() -> Result<Footer>,
{
    Ok(goat_console::cell(title, body)?)
}

pub async fn cell_async<F, Fut>(title: &str, body: F) -> Result<Settled>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<Footer>>,
{
    Ok(goat_console::cell_async(title, body).await?)
}

pub fn within<T>(title: &str, result: Result<T>) -> Result<T> {
    result.map_err(|error| {
        let _ = goat_console::cell(title, || Err::<Footer, _>(error));
        Reported.into()
    })
}

pub fn report(message: impl Into<String>) -> Error {
    anyhow!(format_failure(&message.into(), None))
}

pub fn report_hint(message: impl Into<String>, hint: impl Into<String>) -> Error {
    anyhow!(format_failure(&message.into(), Some(hint.into())))
}

pub fn fail(message: impl Into<String>) -> Result<()> {
    Err(report(message))
}

pub fn fail_hint(message: impl Into<String>, hint: impl Into<String>) -> Result<()> {
    Err(report_hint(message, hint))
}
