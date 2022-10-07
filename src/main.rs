use config::Config;
use rand::{
    distributions::{Distribution, Standard},
    Rng,
};
use serde::Deserialize;
use std::collections::HashMap;
use twitch_irc::login::StaticLoginCredentials;
use twitch_irc::message::ServerMessage;
use twitch_irc::ClientConfig;
use twitch_irc::SecureTCPTransport;
use twitch_irc::TwitchIRCClient;

#[derive(Debug, Deserialize)]
struct BotConfig {
    name: String,
    oauth_token: String,
    channels: Vec<String>,
}

enum Action {
    DoNothing,
    Steal,
    Destroy,
}
impl Distribution<Action> for Standard {
    fn sample<R: Rng + ?Sized>(&self, rng: &mut R) -> Action {
        match rng.gen_range(0..=2) {
            0 => Action::DoNothing,
            1 => Action::Steal,
            _ => Action::Destroy,
        }
    }
}

async fn do_something(
    client: &TwitchIRCClient<SecureTCPTransport, StaticLoginCredentials>,
    channel: &str,
    emote: &str,
) {
    let action: Action = rand::random();
    match action {
        Action::Steal => {
            client
                .say(channel.to_string(), emote.to_string())
                .await
                .unwrap();
        }
        Action::Destroy => {
            client
                .say(channel.to_string(), "No".to_owned())
                .await
                .unwrap();
        }
        _ => (),
    }
}

#[tokio::main]
pub async fn main() {
    let settings = Config::builder()
        .add_source(config::File::with_name("settings.toml"))
        .build()
        .expect("Need the config");

    let conf = settings
        .try_deserialize::<BotConfig>()
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
        building_flags.insert(channel_to_connect.clone(), false);
        emote_counts.insert(channel_to_connect.clone(), 0usize);
        emotes.insert(channel_to_connect.clone(), "".to_owned());
        pyramid_count.insert(channel_to_connect.clone(), HashMap::new());
    }

    let cl = client.clone();
    let join_handle = tokio::spawn(async move {
        while let Some(message) = incoming_messages.recv().await {
            match message {
                ServerMessage::Privmsg(msg) => {
                    let channel = msg.channel_login;
                    let pyramid_building = building_flags.get_mut(channel.as_str()).unwrap();
                    let emote_count = emote_counts.get_mut(channel.as_str()).unwrap();
                    let emote = emotes.get_mut(channel.as_str()).unwrap();
                    println!("(#{}) {}: {}", channel, msg.sender.name, msg.message_text);
                    if msg.message_text.to_lowercase().contains("ayy") {
                        say_rate_limited(&cl, &lim,channel.clone(), "lmao").await;
                    }

                    if msg.sender.name == "StreamElements" && msg.message_text.contains("pirámide") {
                        let chat_count = pyramid_count.get_mut(channel.as_str()).unwrap();
                        let as_vec = msg.message_text.split(" ").collect::<Vec<_>>();
                        let name = as_vec[as_vec.len() - 2];
                        let num = chat_count.entry(name.to_string()).or_insert(0u8);
                        *num += 1;
                        cl.say(channel.clone(), format!("{} lleva {} piramides", name, *num)).await.unwrap();
                    }

                    if !msg.message_text.contains(" ") {
                        *pyramid_building = true;
                        *emote_count = 1;
                        *emote = msg.message_text;
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
                            continue;
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
                                    do_something(&cl, channel.as_str(), &emote).await;
                                    *pyramid_building = false;
                                }
                            }
                            _ => *pyramid_building = false,
                        }
                    } else {
                        *pyramid_building = false;
                    }
                }
                _ => {
                    println!("{:?}", message)
                }
            }
        }
    });

    for channel_to_connect in &conf.channels {
        client.join(channel_to_connect.clone()).unwrap();
    }

    // keep the tokio executor alive.
    // If you return instead of waiting the background task will exit.
    join_handle.await.unwrap();
}
