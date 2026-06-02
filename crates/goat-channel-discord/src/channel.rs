use std::collections::HashSet;
use std::sync::Arc;

use async_trait::async_trait;
use goat_channel::{
    BindOutput, Channel, ChannelBinding, ChannelError, ChannelHandle, ChannelIdentity,
    ChannelResult,
};
use goat_command::{CommandArgs, CommandSpec};
use goat_types::{ChannelId, PersonaId};
use tokio::sync::mpsc;
use tracing::{info, warn};
use twilight_gateway::{Intents, Shard, ShardId};
use twilight_http::Client as HttpClient;
use twilight_model::application::command::{
    Command, CommandOption, CommandOptionType, CommandType,
};
use twilight_model::id::marker::ApplicationMarker;
use twilight_model::id::Id;

use crate::config::DiscordConfig;
use crate::handle::DiscordHandle;
use crate::inbound::{gateway_loop, GatewayConfig};
use crate::interaction::InteractionState;
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
        let commands = binding.commands;
        let http = Arc::new(HttpClient::new(cfg.token.clone()));
        let me = http
            .current_user()
            .await
            .map_err(|e| ChannelError::Auth(e.to_string()))?
            .model()
            .await
            .map_err(|e| ChannelError::Auth(e.to_string()))?;
        let application_id: Id<ApplicationMarker> = Id::new(me.id.get());

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
        let allowed_user_ids: HashSet<u64> = cfg.allowed_user_ids.iter().copied().collect();
        let shard = Shard::new(ShardId::ONE, cfg.token, intents);

        let (tx, rx) = mpsc::channel(INCOMING_CAPACITY);
        let interactions = Arc::new(InteractionState::default());
        register_commands(http.clone(), application_id, &commands).await;
        tokio::spawn(gateway_loop(
            shard,
            http.clone(),
            tx,
            GatewayConfig {
                persona,
                instance: binding.instance,
                commands,
                interactions: interactions.clone(),
                allowed_user_ids,
            },
        ));

        info!(persona = %persona, "discord bot bound: {}", identity.handle);
        let handle: Arc<dyn ChannelHandle> = Arc::new(DiscordHandle::new(
            binding.instance,
            persona,
            identity,
            http,
            interactions,
        ));
        Ok((handle, rx))
    }
}

async fn register_commands(
    http: Arc<HttpClient>,
    application_id: Id<ApplicationMarker>,
    specs: &[CommandSpec],
) {
    if specs.is_empty() {
        return;
    }
    let mut seen = HashSet::new();
    let commands = specs
        .iter()
        .filter_map(discord_command)
        .filter(|command| seen.insert(command.name.clone()))
        .collect::<Vec<_>>();
    if commands.is_empty() {
        return;
    }
    let command_count = commands.len();
    if let Err(e) = http
        .interaction(application_id)
        .set_global_commands(&commands)
        .await
    {
        warn!(error = ?e, "discord command registration failed");
    } else {
        info!(command_count, "discord commands registered");
    }
}

fn discord_command(spec: &CommandSpec) -> Option<Command> {
    Some(Command {
        application_id: None,
        contexts: None,
        default_member_permissions: None,
        #[allow(deprecated)]
        dm_permission: Some(true),
        description: command_description(&spec.description),
        description_localizations: None,
        guild_id: None,
        id: None,
        integration_types: None,
        kind: CommandType::ChatInput,
        name: crate::inbound::discord_command_name(spec.name.as_str())?,
        name_localizations: None,
        nsfw: None,
        options: command_options(&spec.args),
        version: Id::new(1),
    })
}

fn command_options(args: &CommandArgs) -> Vec<CommandOption> {
    match args {
        CommandArgs::None => Vec::new(),
        CommandArgs::RawString {
            name,
            description,
            required,
        } => vec![CommandOption {
            autocomplete: None,
            channel_types: None,
            choices: None,
            description: command_description(description),
            description_localizations: None,
            kind: CommandOptionType::String,
            max_length: Some(6000),
            max_value: None,
            min_length: None,
            min_value: None,
            name: discord_option_name(name),
            name_localizations: None,
            options: None,
            required: Some(*required),
        }],
        _ => Vec::new(),
    }
}

fn discord_option_name(name: &str) -> String {
    crate::inbound::discord_command_name(name).unwrap_or_else(|| "args".to_string())
}

fn command_description(description: &str) -> String {
    let trimmed = description.trim();
    if trimmed.is_empty() {
        return "Run command".to_string();
    }
    let mut out = trimmed.chars().take(100).collect::<String>();
    if out.len() < 3 {
        out = "Run command".to_string();
    }
    out
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
