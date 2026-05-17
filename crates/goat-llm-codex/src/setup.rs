use async_trait::async_trait;
use goat_llm::{Setup, SetupCtx, SetupError};
use serde_json::Value;

use crate::auth;

pub(crate) struct PkceSetup;

#[async_trait]
impl Setup for PkceSetup {
    fn description(&self) -> &str {
        "ChatGPT login (browser PKCE)"
    }

    async fn run(&self, ctx: SetupCtx) -> Result<Value, SetupError> {
        let pkce = auth::generate_pkce();
        let state = auth::random_nonce();
        let (server, port) = auth::bind_listener().map_err(SetupError::Other)?;
        let redirect_uri = auth::redirect_uri_for(port);
        let url = auth::authorize_url(&pkce.challenge, &state, &redirect_uri);

        ctx.prompt.info(&format!(
            "Opening browser for ChatGPT login on port {port}…"
        ));
        if let Err(e) = open::that(&url) {
            ctx.prompt.info(&format!(
                "(browser launch failed: {e}; open this URL manually)"
            ));
        }
        ctx.prompt.info(&format!("Or visit: {url}"));

        let verifier = pkce.verifier.clone();
        let expected_state = state.clone();
        let code =
            tokio::task::spawn_blocking(move || auth::await_callback(server, &expected_state))
                .await
                .map_err(|e| SetupError::Other(format!("listener join: {e}")))?
                .map_err(SetupError::Other)?;

        let cred = auth::exchange_code(&code, &verifier, &redirect_uri, ctx.label.clone())
            .await
            .map_err(SetupError::Other)?;

        if cred.account_id.is_none() {
            return Err(SetupError::Other(
                "ChatGPT account_id missing from id_token — ChatGPT Plus or higher required".into(),
            ));
        }

        serde_json::to_value(&cred).map_err(|e| SetupError::Other(e.to_string()))
    }
}
