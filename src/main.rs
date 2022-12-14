use config::Config;
use simple_logger::SimpleLogger;
use tokio::sync::mpsc;
use twitch_irc::login::StaticLoginCredentials;
use twitch_irc::ClientConfig;
use twitch_irc::SecureWSTransport;
use twitch_irc::TwitchIRCClient;
use twitch_piramid_bot::bot_config::BotConfig;
use twitch_piramid_bot::chat_loop::message_loop;
use twitch_piramid_bot::event_loop::create_event_loop;
use twitch_piramid_bot::state_manager::create_manager;

#[tokio::main]
pub async fn main() {
    SimpleLogger::new().env().init().unwrap();
    let settings = Config::builder()
        .add_source(config::File::with_name("settings.toml"))
        .build()
        .expect("Need the config");

    let conf = settings
        .try_deserialize::<BotConfig>()
        .expect("Malformed config");

    let twitch_config = ClientConfig::new_simple(StaticLoginCredentials::new(
        conf.name.clone(),
        Some(conf.oauth_token.clone()),
    ));
    let (incoming_messages, client) =
        TwitchIRCClient::<SecureWSTransport, StaticLoginCredentials>::new(twitch_config);
    //client.send_message(IRCMessage::parse("CAP REQ :twitch.tv/commands twitch.tv/tags").unwrap());

    let (tx, rx) = mpsc::channel(32);
    let _manager = create_manager(&conf, rx);
    let _event_loop = create_event_loop(&conf, tx.clone());
    let join_handle = message_loop(&conf, incoming_messages, client.clone(), tx.clone());

    for channel_to_connect in &conf.channels {
        client
            .join(channel_to_connect.channel_name.clone())
            .unwrap();
    }

    // keep the tokio executor alive.
    // If you return instead of waiting the background task will exit.
    join_handle.await.unwrap();
}
