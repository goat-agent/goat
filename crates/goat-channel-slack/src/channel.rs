use std::collections::HashSet;
use std::sync::Arc;

use async_trait::async_trait;
use goat_channel::{
    BindOutput, Channel, ChannelBinding, ChannelError, ChannelHandle, ChannelIdentity,
    ChannelResult, ChannelSecrets,
};
use goat_types::{ChannelId, ProfileId};
use tokio::sync::mpsc;
use tracing::{info, warn};

use crate::api::{Identity, SlackApi};
use crate::config::SlackConfig;
use crate::handle::SlackHandle;
use crate::inbound::{SocketConfig, socket_loop};
use crate::{APP_TOKEN_SLOT, BOT_TOKEN_SLOT, ID};

const INCOMING_CAPACITY: usize = 256;

#[derive(Default)]
pub struct SlackChannel;

#[async_trait]
impl Channel for SlackChannel {
    fn id(&self) -> ChannelId {
        ID.clone()
    }

    async fn bind(
        self: Arc<Self>,
        persona: ProfileId,
        binding: ChannelBinding,
    ) -> ChannelResult<BindOutput> {
        install_crypto_provider();
        let bot_token = binding.secrets.require(BOT_TOKEN_SLOT)?.to_owned();
        let app_token = binding.secrets.require(APP_TOKEN_SLOT)?.to_owned();
        let cfg: SlackConfig = serde_json::from_value(binding.config)
            .map_err(|e| ChannelError::Config(format!("slack: {e}")))?;

        let api = Arc::new(SlackApi::new(bot_token)?);
        let whoami = api.auth_test().await?;
        let identity = identity_of(&whoami, api.as_ref()).await;

        let allowed_user_ids: HashSet<String> = cfg.allowed_user_ids.iter().cloned().collect();
        if allowed_user_ids.len() != cfg.allowed_user_ids.len() {
            warn!("slack: allowed_user_ids contains duplicate entries; deduplicated");
        }

        let (tx, rx) = mpsc::channel(INCOMING_CAPACITY);
        tokio::spawn(socket_loop(
            SocketConfig {
                persona,
                instance: binding.instance,
                commands: binding.commands,
                allowed_user_ids,
                bot_user_id: whoami.user_id.clone(),
                bot_id: whoami.bot_id.clone(),
                app_token,
                api: api.clone(),
            },
            tx,
        ));

        info!(profile = %persona, "slack bot bound: {}", identity.handle);
        let handle: Arc<dyn ChannelHandle> =
            Arc::new(SlackHandle::new(binding.instance, persona, identity, api));
        Ok((handle, rx))
    }

    async fn verify(
        &self,
        config: &serde_json::Value,
        secrets: &ChannelSecrets,
    ) -> ChannelResult<ChannelIdentity> {
        install_crypto_provider();
        serde_json::from_value::<SlackConfig>(config.clone())
            .map_err(|e| ChannelError::Config(format!("slack: {e}")))?;
        let api = SlackApi::new(secrets.require(BOT_TOKEN_SLOT)?.to_owned())?;
        let whoami = api.auth_test().await?;
        crate::api::open_connection(secrets.require(APP_TOKEN_SLOT)?).await?;
        Ok(identity_of(&whoami, &api).await)
    }
}

fn install_crypto_provider() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

async fn identity_of(whoami: &Identity, api: &SlackApi) -> ChannelIdentity {
    let display = whoami.team.as_ref().map_or_else(
        || whoami.user.clone(),
        |team| format!("{} @ {team}", whoami.user),
    );
    let mut identity = ChannelIdentity::new(whoami.user.clone(), display);
    if let Ok(profile) = api.user_profile(&whoami.user_id).await
        && let Some(avatar) = profile.avatar.as_deref().and_then(|url| url.parse().ok())
    {
        identity = identity.with_avatar(avatar);
    }
    identity
}
