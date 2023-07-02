use config::Config;
use simple_logger::SimpleLogger;
use std::sync::Arc;
use tokio::sync::mpsc;
use twitch_irc::login::{RefreshingLoginCredentials, StaticLoginCredentials, TokenStorage};
use twitch_irc::ClientConfig;
use twitch_irc::SecureWSTransport;
use twitch_irc::TwitchIRCClient;
use twitch_piramid_bot::bot_config::BotConfig;
use twitch_piramid_bot::bot_token_storage::CustomTokenStorage;
use twitch_piramid_bot::chat_loop::message_loop;
use twitch_piramid_bot::event_loop::create_event_loop;
use twitch_piramid_bot::state_manager::create_manager;

#[tokio::main]
pub async fn main() {
    SimpleLogger::new()
        .env()
        .init()
        .expect("Could not init logger");
    let settings = Config::builder()
        .add_source(config::File::with_name("settings.toml"))
        .build()
        .expect("Need the config");

    let conf = settings
        .try_deserialize::<BotConfig>()
        .expect("Malformed config");

    let storage = CustomTokenStorage {
        location: conf.credentials_file.clone(),
    };
    let credentials = RefreshingLoginCredentials::init(
        conf.client_id.clone(),
        conf.client_secret.clone(),
        storage,
    );

    let twitch_config = ClientConfig::new_simple(credentials);
    let (incoming_messages, client) = TwitchIRCClient::<
        SecureWSTransport,
        RefreshingLoginCredentials<CustomTokenStorage>,
    >::new(twitch_config);
    //client.send_message(IRCMessage::parse("CAP REQ :twitch.tv/commands twitch.tv/tags").unwrap());

    let (tx, rx) = mpsc::channel(32);
    let conf = Arc::new(conf);
    let _manager = create_manager(conf.clone(), rx);
    let _event_loop = create_event_loop(conf.clone(), tx.clone());
    let join_handle = message_loop(&conf, incoming_messages, client.clone(), tx.clone());

    for channel_to_connect in &conf.channels {
        client
            .join(channel_to_connect.channel_name.clone())
            .expect("Could not connect to a channel");
    }

    // keep the tokio executor alive.
    // If you return instead of waiting the background task will exit.
    join_handle.await.unwrap();
}
