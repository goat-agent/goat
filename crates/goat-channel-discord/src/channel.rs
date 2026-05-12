use std::sync::Arc;

use async_trait::async_trait;
use goat_channel::{
    BindOutput, Channel, ChannelBinding, ChannelError, ChannelHandle, ChannelIdentity,
    ChannelResult,
};
use goat_types::{ChannelId, PersonaId};
use tokio::sync::mpsc;
use tracing::{info, warn};
use twilight_gateway::{Intents, Shard, ShardId};
use twilight_http::Client as HttpClient;

use crate::config::DiscordConfig;
use crate::handle::DiscordHandle;
use crate::inbound::gateway_loop;
use crate::ID;

const INCOMING_CAPACITY: usize = 256;

#[derive(Default)]
pub struct DiscordChannel;

#[async_trait]
impl Channel for DiscordChannel {
    fn id(&self) -> ChannelId {
        ID.clone()
    }

    async fn bind(
        self: Arc<Self>,
        persona: PersonaId,
        binding: ChannelBinding,
    ) -> ChannelResult<BindOutput> {
        let cfg: DiscordConfig = serde_json::from_value(binding.config)
            .map_err(|e| ChannelError::Config(format!("discord: {e}")))?;
        let http = Arc::new(HttpClient::new(cfg.token.clone()));
        let me = http
            .current_user()
            .await
            .map_err(|e| ChannelError::Auth(e.to_string()))?
            .model()
            .await
            .map_err(|e| ChannelError::Auth(e.to_string()))?;

        let mut identity = ChannelIdentity::new(me.name.clone(), me.name.clone());
        if let Some(hash) = me.avatar {
            let url_str = format!("https://cdn.discordapp.com/avatars/{}/{}.png", me.id, hash);
            if let Ok(url) = url::Url::parse(&url_str) {
                identity = identity.with_avatar(url);
            }
        }

        let intents = if cfg.intents.is_empty() {
            Intents::GUILD_MESSAGES | Intents::DIRECT_MESSAGES | Intents::MESSAGE_CONTENT
        } else {
            parse_intents(&cfg.intents)
        };
        let shard = Shard::new(ShardId::ONE, cfg.token, intents);

        let (tx, rx) = mpsc::channel(INCOMING_CAPACITY);
        tokio::spawn(gateway_loop(shard, persona, binding.instance, tx));

        info!(persona = %persona, "discord bot bound: {}", identity.handle);
        let handle: Arc<dyn ChannelHandle> = Arc::new(DiscordHandle::new(
            binding.instance,
            persona,
            identity,
            http,
        ));
        Ok((handle, rx))
    }
}

fn parse_intents(names: &[String]) -> Intents {
    let mut out = Intents::empty();
    for name in names {
        match name.as_str() {
            "GUILD_MESSAGES" => out |= Intents::GUILD_MESSAGES,
            "DIRECT_MESSAGES" => out |= Intents::DIRECT_MESSAGES,
            "MESSAGE_CONTENT" => out |= Intents::MESSAGE_CONTENT,
            "GUILDS" => out |= Intents::GUILDS,
            other => warn!(intent = %other, "unknown discord intent in config"),
        }
    }
    out
}
