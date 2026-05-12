use std::sync::Arc;

use async_trait::async_trait;
use goat_channel::{
    BindOutput, Channel, ChannelBinding, ChannelError, ChannelHandle, ChannelIdentity,
    ChannelResult,
};
use goat_types::{ChannelId, PersonaId};
use teloxide::prelude::*;
use teloxide::Bot;
use tokio::sync::mpsc;
use tracing::info;

use crate::config::TelegramConfig;
use crate::handle::TelegramHandle;
use crate::inbound::poll_loop;
use crate::ID;

const INCOMING_CAPACITY: usize = 256;

#[derive(Default)]
pub struct TelegramChannel;

#[async_trait]
impl Channel for TelegramChannel {
    fn id(&self) -> ChannelId {
        ID.clone()
    }

    async fn bind(
        self: Arc<Self>,
        persona: PersonaId,
        binding: ChannelBinding,
    ) -> ChannelResult<BindOutput> {
        let cfg: TelegramConfig = serde_json::from_value(binding.config)
            .map_err(|e| ChannelError::Config(format!("telegram: {e}")))?;
        let _allowed_user_ids = cfg.allowed_user_ids;
        let bot = Bot::new(cfg.token);
        let me = bot
            .get_me()
            .await
            .map_err(|e| ChannelError::Auth(e.to_string()))?;
        let identity = ChannelIdentity::new(me.username().to_string(), me.full_name());

        let (tx, rx) = mpsc::channel(INCOMING_CAPACITY);
        let handle: Arc<dyn ChannelHandle> = Arc::new(TelegramHandle::new(
            binding.instance,
            persona,
            identity.clone(),
            bot.clone(),
        ));

        tokio::spawn(poll_loop(bot, persona, binding.instance, tx));
        info!(persona = %persona, "telegram bot bound: @{}", identity.handle);
        Ok((handle, rx))
    }
}
