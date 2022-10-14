use crate::chat_action::ChatAction;
use crate::pyramid_action::PyramidAction;
use config::Config;
use governor::clock::DefaultClock;
use governor::state::keyed::DefaultKeyedStateStore;
use governor::{Quota, RateLimiter};
use std::collections::HashMap;
use std::num::NonZeroU32;
use std::time::Duration;
use twitch_irc::login::StaticLoginCredentials;
use twitch_irc::message::ServerMessage;
use twitch_irc::ClientConfig;
use twitch_irc::SecureTCPTransport;
use twitch_irc::TwitchIRCClient;

mod bot_config;
mod chat_action;
mod pyramid_action;

type Client = TwitchIRCClient<SecureTCPTransport, StaticLoginCredentials>;
type Limiter = RateLimiter<String, DefaultKeyedStateStore<String>, DefaultClock>;

struct Combo {
    client: Client,
    limiter: Limiter,
}

async fn say_rate_limited(combo: &Combo, channel: String, msg: &str) {
    match combo.limiter.check_key(&channel) {
        Ok(_) => combo.client.say(channel, msg.to_string()).await.unwrap(),
        Err(_) => {
            println!("Rate limited")
        }
    }
}

async fn do_ayy(combo: &Combo, msg: &twitch_irc::message::PrivmsgMessage) {
    if msg.message_text.to_lowercase().contains("ayy") {
        say_rate_limited(combo, msg.channel_login.clone(), "lmao").await;
    }
}

async fn do_pyramid_counting(
    combo: &Combo,
    msg: &twitch_irc::message::PrivmsgMessage,
    pyramid_count: &mut HashMap<String, HashMap<String, u8>>,
) {
    if msg.sender.name == "StreamElements" && msg.message_text.contains("pirámide") {
        let chat_count = pyramid_count.get_mut(msg.channel_login.as_str()).unwrap();
        let as_vec = msg.message_text.split(" ").collect::<Vec<_>>();
        let name = as_vec[as_vec.len() - 2];
        let num = chat_count.entry(name.to_string()).or_insert(0u8);
        *num += 1;
        let message = format!("{} lleva {} piramides", name, *num);
        say_rate_limited(combo, msg.channel_login.clone(), message.as_str()).await;
    }
}

async fn do_pyramid_interference(
    combo: &Combo,
    msg: &twitch_irc::message::PrivmsgMessage,
    building_flags: &mut HashMap<String, bool>,
    emote_counts: &mut HashMap<String, usize>,
    emotes: &mut HashMap<String, String>,
) {
    let channel = &msg.channel_login;
    let pyramid_building = building_flags.get_mut(channel.as_str()).unwrap();
    let emote_count = emote_counts.get_mut(channel.as_str()).unwrap();
    let emote = emotes.get_mut(channel.as_str()).unwrap();

    if !msg.message_text.contains(" ") {
        *pyramid_building = true;
        *emote_count = 1;
        *emote = msg.message_text.clone();
        println!("Single word {}", *emote);
    } else if *pyramid_building {
        let num_of_matches = msg
            .message_text
            .match_indices(emote.as_str())
            .collect::<Vec<_>>()
            .len();
        let num_of_words = msg.message_text.split(" ").collect::<Vec<_>>().len();
        if num_of_words != num_of_matches {
            *pyramid_building = false;
            return;
        }
        match num_of_matches {
            i if i == *emote_count + 1 => {
                println!("Pyramid growing");
                *emote_count += 1;
            }
            i if i == *emote_count - 1 => {
                println!("Pyramid getting smaller");
                *emote_count -= 1;
                if *emote_count == 2 {
                    println!("Time to strike");
                    do_pyramid_action(combo, channel.as_str(), &emote).await;
                    *pyramid_building = false;
                }
            }
            _ => *pyramid_building = false,
        }
    } else {
        *pyramid_building = false;
    }
}

async fn do_pyramid_action(combo: &Combo, channel: &str, emote: &str) {
    let action: PyramidAction = rand::random();
    match action {
        PyramidAction::Steal => {
            say_rate_limited(combo, channel.to_string(), emote).await;
        }
        PyramidAction::Destroy => {
            say_rate_limited(combo, "No".to_string(), emote).await;
        }
        _ => println!("Do nothing"),
    }
}

#[tokio::main]
pub async fn main() {
    let settings = Config::builder()
        .add_source(config::File::with_name("settings.toml"))
        .build()
        .expect("Need the config");

    let conf = settings
        .try_deserialize::<bot_config::BotConfig>()
        .expect("Malformed config");

    let twitch_config = ClientConfig::new_simple(StaticLoginCredentials::new(
        conf.name,
        Some(conf.oauth_token),
    ));
    let (mut incoming_messages, client) =
        TwitchIRCClient::<SecureTCPTransport, StaticLoginCredentials>::new(twitch_config);

    let mut building_flags = HashMap::new();
    let mut emote_counts = HashMap::new();
    let mut emotes = HashMap::new();
    let mut pyramid_count = HashMap::new();

    for channel_to_connect in &conf.channels {
        let channel_name = &channel_to_connect.channel_name;
        building_flags.insert(channel_name.clone(), false);
        emote_counts.insert(channel_name.clone(), 0usize);
        emotes.insert(channel_name.clone(), "".to_owned());
        pyramid_count.insert(channel_name.clone(), HashMap::new());
    }

    let cl = client.clone();
    let channel_confs = conf.channels.iter().fold(HashMap::new(), |mut acc, c| {
        *acc.entry(c.channel_name.clone()).or_default() = c
            .permitted_actions
            .iter()
            .map(|c| c.clone())
            .collect::<Vec<_>>();
        acc
    });
    let join_handle = tokio::spawn(async move {
        let lim = RateLimiter::keyed(
            Quota::with_period(Duration::from_secs_f32(1.5))
                .unwrap()
                .allow_burst(NonZeroU32::new(20).unwrap()),
        );

        let combo = Combo {
            client: cl,
            limiter: lim,
        };
        while let Some(message) = incoming_messages.recv().await {
            match message {
                ServerMessage::Privmsg(msg) => {
                    println!(
                        "(#{}) {}: {}",
                        msg.channel_login, msg.sender.name, msg.message_text
                    );

                    let channel_conf = channel_confs.get(&msg.channel_login).unwrap();

                    for action in channel_conf {
                        match action {
                            ChatAction::Ayy => {
                                do_ayy(&combo, &msg).await;
                            }
                            ChatAction::PyramidCounting => {
                                do_pyramid_counting(&combo, &msg, &mut pyramid_count).await;
                            }
                            ChatAction::PyramidInterference => {
                                do_pyramid_interference(
                                    &combo,
                                    &msg,
                                    &mut building_flags,
                                    &mut emote_counts,
                                    &mut emotes,
                                )
                                .await;
                            }
                        }
                    }
                }
                _ => {
                    println!("{:?}", message)
                }
            }
        }
    });

    for channel_to_connect in &conf.channels {
        client
            .join(channel_to_connect.channel_name.clone())
            .unwrap();
    }

    // keep the tokio executor alive.
    // If you return instead of waiting the background task will exit.
    join_handle.await.unwrap();
}
