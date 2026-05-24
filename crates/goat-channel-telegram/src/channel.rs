use std::collections::HashSet;
use std::sync::Arc;

use async_trait::async_trait;
use goat_channel::{
    BindOutput, Channel, ChannelBinding, ChannelError, ChannelHandle, ChannelIdentity,
    ChannelResult,
};
use goat_command::{CommandArgs, CommandSpec};
use goat_types::{ChannelId, PersonaId};
use teloxide::prelude::*;
use teloxide::types::BotCommand;
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
        let allowed_user_ids: HashSet<i64> = cfg.allowed_user_ids.into_iter().collect();
        let commands = binding.commands;
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

        if !commands.is_empty() {
            let mut seen = HashSet::new();
            let bot_commands = commands
                .iter()
                .filter_map(|command| telegram_bot_command(command, &mut seen))
                .collect::<Vec<_>>();
            if !bot_commands.is_empty() {
                let command_count = bot_commands.len();
                if let Err(e) = bot.set_my_commands(bot_commands).send().await {
                    tracing::warn!(error = ?e, "telegram command registration failed");
                } else {
                    info!(persona = %persona, command_count, "telegram commands registered");
                }
            }
        }

        tokio::spawn(poll_loop(
            bot,
            persona,
            binding.instance,
            tx,
            commands,
            allowed_user_ids,
        ));
        info!(persona = %persona, "telegram bot bound: @{}", identity.handle);
        Ok((handle, rx))
    }
}

fn telegram_bot_command(command: &CommandSpec, seen: &mut HashSet<String>) -> Option<BotCommand> {
    let name = crate::inbound::telegram_command_name(command.name.as_str())?;
    let description = match &command.args {
        CommandArgs::None => truncate_description(&command.description),
        CommandArgs::RawString { .. } => truncate_description(&command.description),
        _ => truncate_description(&command.description),
    };
    seen.insert(name.clone())
        .then(|| BotCommand::new(name, description))
}

fn truncate_description(description: &str) -> String {
    let trimmed = description.trim();
    if trimmed.len() < 3 {
        return "Run command".to_string();
    }
    if trimmed.len() <= 256 {
        return trimmed.to_string();
    }
    trimmed.chars().take(253).collect::<String>() + "..."
}
