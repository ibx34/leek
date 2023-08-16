#![feature(let_chains)]
use clap::{Arg, ArgAction, Command};
use core::future::Future;
use futures_util::StreamExt;
use handlebars::Handlebars;
use once_cell::sync::Lazy;
use rand::Rng;
use serde::{Deserialize, Serialize};
use std::pin::Pin;
use std::{collections::HashMap, fs::read_to_string, sync::Arc};
use tokio::sync::Mutex;
use twilight_cache_inmemory::{InMemoryCache, ResourceType};
use twilight_gateway::{
    stream::{self, ShardEventStream},
    Config, Intents,
};
use twilight_http::{request::channel::reaction::RequestReactionType, Client};
use twilight_model::channel::ChannelType;
use twilight_model::guild::Permissions;
use twilight_model::id::marker::{ChannelMarker, MessageMarker, RoleMarker};
use twilight_model::id::Id;
use twilight_util::permission_calculator::PermissionCalculator;

#[derive(Serialize, Deserialize, Debug)]
pub struct LeekConfig {
    pub token: String,
}

pub struct Leek {
    pub client: Arc<Client>,
    pub cache: Arc<InMemoryCache>,
}

#[derive(Serialize, Deserialize)]
pub struct BanMessagedata {
    pub user_id: u64,
    pub username: String,
}

#[derive(Clone)]
pub struct LeekCommand {
    pub permissions: Option<Permissions>,
    pub command: Command,
}

const COMMANDS: Lazy<HashMap<&str, LeekCommand>> = Lazy::new(|| {
    let mut commands = HashMap::new();
    commands.insert(
        "ban",
        LeekCommand {
            permissions: Some(Permissions::BAN_MEMBERS),
            command: Command::new("ban").arg(
                Arg::new("user")
                    .required(true)
                    .value_parser(clap::value_parser!(u64).range(0..))
                    .aliases(["target"]),
            ).arg(Arg::new("soft").short('s').long("soft").action(ArgAction::SetTrue).help("Whether or not the ban should be \"soft\". Soft bans are bans that ban the user to quickly remove ALL their messages.")),
        },
    );
    commands.insert(
        "clear",
        LeekCommand {
            permissions: Some(Permissions::MANAGE_MESSAGES),
            command: Command::new("clear")
                .arg(
                    Arg::new("before")
                        .short('b')
                        .long("before")
                        .value_parser(clap::value_parser!(u64).range(0..))
                        .help("Only clears messages before the specified message"),
                )
                .arg(
                    Arg::new("after")
                        .long("after")
                        .value_parser(clap::value_parser!(u64).range(0..))
                        .help("Only clears messages after the specified message"),
                )
                .arg(
                    Arg::new("amount")
                        .aliases(["amt"])
                        .short('a')
                        .long("amount")
                        .value_parser(clap::value_parser!(u16).range(1..1001)),
                ),
        },
    );
    commands
});

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .pretty()
        .init();

    let mut handlebars = Handlebars::new();
    assert!(handlebars
        .register_template_string("ban_1", "**{{ username }}** has been packed up")
        .is_ok());
    assert!(handlebars
        .register_template_string(
            "ban_2",
            "**{{ username }}** broke their legs while jumping over a highwall"
        )
        .is_ok());

    let config_file = read_to_string("./Config.yml")?;
    let config: LeekConfig = serde_yaml::from_str(&config_file)?;

    let token = config.token.clone();
    let client = Arc::new(Client::new(token.to_owned()));

    let resource_types = ResourceType::CHANNEL | ResourceType::MEMBER | ResourceType::ROLE;
    let cache = Arc::new(
        InMemoryCache::builder()
            .resource_types(resource_types)
            .message_cache_size(10)
            .build(),
    );

    let config = Config::new(token.to_owned(), Intents::all());

    let mut shards = stream::create_recommended(&client, config, |_, builder| builder.build())
        .await?
        .collect::<Vec<_>>();

    let mut stream = ShardEventStream::new(shards.iter_mut());

    while let Some((_shard, Ok(event))) = stream.next().await {
        cache.update(&event);
        let handlebars = handlebars.clone();
        let client = client.clone();
        // let event = match event {
        //     Ok(event) => event,
        //     Err(source) => {
        //         tracing::warn!(?source, "error receiving event");
        //         if source.is_fatal() {
        //             break;
        //         }
        //         continue;
        //     }
        // };
        let cache = cache.clone();
        tokio::task::spawn(async move {
            match event {
                twilight_gateway::Event::MessageCreate(message) => {
                    let message = *message;
                    if let Some(guild_id) = message.guild_id {
                        let message_content = &message.content;
                        if message_content.starts_with("leek") {
                            let stripped_message_content = message_content[4..].to_string();
                            let split_stripped_message_content = stripped_message_content
                                .trim()
                                .split_ascii_whitespace()
                                .collect::<Vec<&str>>();

                            if split_stripped_message_content.len() >= 1 && let Some(first) = split_stripped_message_content.get(0) {
                                let first = first.to_owned();
                                if let Some(cmd) = COMMANDS.to_owned().get(first) {
                                    let permissions = cache.clone().permissions().in_channel(message.author.id, message.channel_id).unwrap();

                                    if let Some(cmd_perms) = cmd.permissions {
                                        if !permissions.contains(cmd_perms)  {
                                            client.create_reaction(message.channel_id, message.id,  &RequestReactionType::Unicode { name: "❌" }).await.unwrap();
                                        }
                                    }
    
                                    match cmd.command.clone().try_get_matches_from(split_stripped_message_content) {
                                        Ok(matched) => match first {
                                            "clear" | "purge" => {
                                                let mut purge_stats: HashMap<(u64,String),u64> = HashMap::new();
                                                let mut overall_count = 0;
                                                client.create_reaction(message.channel_id, message.id,  &RequestReactionType::Unicode { name: "🚼" }).await.unwrap();                                                 
                                                let amt: &u16 = if let Some(amt) = matched.get_one("amount") {
                                                    amt 
                                                } else {
                                                    &10u16
                                                };
                                                let messages = client.channel_messages(message.channel_id).before(match matched.get_one::<u64>("before") {
                                                    Some(id) => Id::<MessageMarker>::new(*id),
                                                    None => message.id
                                                }).limit(*amt).unwrap().await.unwrap().models().await.unwrap().into_iter().map(|e| {
                                                    let author_name = e.author.name.to_owned();
                                                    let Some(entry) = purge_stats.get_mut(&(e.author.id.get(), author_name.to_owned())) else {
                                                        purge_stats.insert((e.author.id.get(), author_name.to_owned()), 0);
                                                        overall_count += 1;
                                                        return e.id
                                                    };
                                                    *entry += 1;
                                                    overall_count += 1;
                                                    e.id
                                                }).collect::<Vec<Id::<MessageMarker>>>();

                                                let mut cleared_from = String::new();
                                                for (key,val) in purge_stats {
                                                    cleared_from.push_str(&format!("**{}** (`{}`): **{val}**\n", key.1, key.0))
                                                }
                                                client.delete_messages(message.channel_id, &messages).unwrap().await.unwrap();
                                                client.delete_all_reactions(message.channel_id, message.id).await.unwrap();
                                                client.create_message(message.channel_id).content(&format!(":ok_hand: Cleared **{overall_count}** messages.\n{cleared_from}")).unwrap().await.unwrap();                                            }
                                            "ban" => {
                                                let Some(target): Option<&u64> = matched.get_one("user") else {
                                                   return                                                    
                                                };
                                                let victim = client.guild_member(guild_id, Id::new(*target)).await.unwrap().model().await.unwrap();
                                                let Some(moderator) = &message.member else {
                                                    client.create_reaction(message.channel_id, message.id,  &RequestReactionType::Unicode { name: "❌" }).await.unwrap();                                              
                                                    return                                                 
                                                };
                                                let moderator_top_role = moderator.roles
                                                .iter()
                                                .flat_map(|r_id| cache.role(*r_id))
                                                .map(|r| r.position.to_owned())
                                                .max_by(|x, y| x.cmp(y))
                                                .unwrap_or(0);
                                                let victim_top_role = victim.roles
                                                .iter()
                                                .flat_map(|r_id| cache.role(*r_id))
                                                .map(|r| r.position.to_owned())
                                                .max_by(|x, y| x.cmp(y))
                                                .unwrap_or(0);
                                                
                                                if victim_top_role >= moderator_top_role {
                                                    println!("Bad");
                                                    client.create_reaction(message.channel_id, message.id,  &RequestReactionType::Unicode { name: "❌" }).await.unwrap();
                                                    return                                               
                                                }

                                                let ban_message_num = rand::thread_rng().gen_range(1..3);
                                                client.create_message(message.channel_id).content(&handlebars.render(&format!("ban_{ban_message_num}"), &BanMessagedata {
                                                    username: victim.user.name,
                                                    user_id: victim.user.id.get()
                                                }).unwrap()).unwrap().await.unwrap();
                                            },
                                            _ => {}
                                        },
                                        Err(err) => {
                                            let string_err = err.to_string();
                                            // client.delete_messages(channel_id, message_ids)
                                            client.create_message(message.channel_id).content(&string_err).unwrap().await.unwrap();
                                            return
                                        }
                                    }
                                } else {
                                    client.create_reaction(message.channel_id, message.id,  &RequestReactionType::Unicode { name: "⚠" }).await.unwrap();
                                }
                            }
                        }
                    }
                }
                _ => {}
            }
        });
    }

    Ok(())
}
