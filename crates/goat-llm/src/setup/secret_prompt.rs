use async_trait::async_trait;
use serde_json::{json, Value};

use super::{Setup, SetupCtx, SetupError};

pub struct SecretPrompt {
    pub description: &'static str,
    pub json_key: &'static str,
    pub hint: &'static str,
}

#[async_trait]
impl Setup for SecretPrompt {
    fn description(&self) -> &str {
        self.description
    }

    async fn run(&self, ctx: SetupCtx) -> Result<Value, SetupError> {
        let secret = ctx.prompt.secret(self.description, self.hint)?;
        Ok(json!({ self.json_key: secret }))
    }
}
