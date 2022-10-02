use config::Config;
use rand::{
    distributions::{Distribution, Standard},
    Rng,
};
use serde::Deserialize;
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

    let cl = client.clone();
    let join_handle = tokio::spawn(async move {
        let mut pyramid_building = false;
        let mut emote_count = 0;
        let mut emote = "".to_owned();
        while let Some(message) = incoming_messages.recv().await {
            match message {
                ServerMessage::Privmsg(msg) => {
                    println!(
                        "(#{}) {}: {}",
                        msg.channel_login, msg.sender.name, msg.message_text
                    );
                    if !msg.message_text.contains(" ") {
                        pyramid_building = true;
                        emote_count = 1;
                        emote = msg.message_text;
                        println!("Single word {}", emote);
                    } else if pyramid_building {
                        let num_of_matches = msg
                            .message_text
                            .match_indices(&emote)
                            .collect::<Vec<_>>()
                            .len();
                        let num_of_words = msg.message_text.split(" ").collect::<Vec<_>>().len();
                        if num_of_words != num_of_matches {
                            pyramid_building = false;
                            continue;
                        }
                        match num_of_matches {
                            i if i == emote_count + 1 => {
                                println!("Pyramid growing");
                                emote_count += 1;
                            }
                            i if i == emote_count - 1 => {
                                println!("Pyramid getting smaller");
                                emote_count -= 1;
                                if emote_count == 2 {
                                    println!("Time to strike");
                                    do_something(&cl, &msg.channel_login, &emote).await;
                                    pyramid_building = false;
                                }
                            }
                            _ => pyramid_building = false,
                        }
                    } else {
                        pyramid_building = false;
                    }
                }
                _ => {
                    println!("{:?}", message)
                }
            }
        }
    });

    client.join(conf.channels[0].clone()).unwrap();

    // keep the tokio executor alive.
    // If you return instead of waiting the background task will exit.
    join_handle.await.unwrap();
}
