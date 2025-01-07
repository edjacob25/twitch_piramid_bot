use config::Config;
use simple_logger::SimpleLogger;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::mpsc;
use twitch_irc::login::{
    GetAccessTokenResponse, LoginCredentials, RefreshingLoginCredentials, TokenStorage, UserAccessToken,
};
use twitch_irc::ClientConfig;
use twitch_irc::SecureWSTransport;
use twitch_irc::TwitchIRCClient;
use twitch_piramid_bot::bot_config::BotConfig;
use twitch_piramid_bot::bot_token_storage::CustomTokenStorage;
use twitch_piramid_bot::chat_loop::message_loop;
use twitch_piramid_bot::event_loop::create_event_loop;
use twitch_piramid_bot::state_manager::create_state_manager;

#[tokio::main]
pub async fn main() {
    SimpleLogger::new()
        .env()
        .with_level(log::LevelFilter::Info)
        .init()
        .expect("Could not init logger");
    let settings = Config::builder()
        .add_source(config::File::with_name("data/settings.toml"))
        .build()
        .expect("Need the config");

    let mut conf = settings.try_deserialize::<BotConfig>().expect("Malformed config");

    let auth_file_location = Path::new(&conf.credentials_file);
    if auth_file_location.is_relative() {
        conf = BotConfig {
            credentials_file: format!("data/{}", conf.credentials_file),
            ..conf
        };
    }

    create_auth_file(&conf).await;
    let storage = CustomTokenStorage {
        location: conf.credentials_file.clone(),
    };
    let credentials = RefreshingLoginCredentials::init(conf.client_id.clone(), conf.client_secret.clone(), storage);

    _ = credentials.get_credentials().await;

    let twitch_config = ClientConfig::new_simple(credentials);
    let (incoming_messages, client) =
        TwitchIRCClient::<SecureWSTransport, RefreshingLoginCredentials<CustomTokenStorage>>::new(twitch_config);
    //client.send_message(IRCMessage::parse("CAP REQ :twitch.tv/commands twitch.tv/tags").unwrap());

    let (tx, rx) = mpsc::channel(32);
    let conf = Arc::new(conf);
    let _state_manager = create_state_manager(conf.clone(), rx);
    let _event_loop = create_event_loop(conf.clone(), tx.clone());

    for channel_to_connect in &conf.channels {
        client
            .join(channel_to_connect.channel_name.clone())
            .expect("Could not connect to a channel");
    }
    let message_loop = message_loop(conf, incoming_messages, client.clone(), tx.clone());

    // keep the tokio executor alive.
    // If you return instead of waiting the background task will exit.
    message_loop.await.unwrap();
}

pub async fn create_auth_file(config: &BotConfig) {
    let file_location = Path::new(&config.credentials_file);
    if file_location.exists() {
        return;
    }

    println!(
        "No token file detected at {}, creating a new one",
        config.credentials_file
    );

    let link = format!(
        "
    https://id.twitch.tv/oauth2/authorize\
    ?response_type=code\
    &client_id={}\
    &redirect_uri=http://localhost:8000\
    &scope=chat%3Aread+chat%3Aedit+channel%3Aread%3Apredictions\
    &state=c3ab8aa609ea11e793ae92361f002671
    ",
        config.client_id
    );

    println!(
        "Please put this link in your browser, authorize and copy back the code: {}",
        link
    );
    let mut code = String::new();
    let _b = std::io::stdin().read_line(&mut code).expect("Error reading the line");
    let code = code.trim();
    let client = reqwest::Client::new();
    let params = [
        ("client_id", config.client_id.as_str()),
        ("client_secret", config.client_secret.as_str()),
        ("code", code),
        ("grant_type", "authorization_code"),
        ("redirect_uri", "http://localhost:8000"),
    ];

    let res = client
        .post("https://id.twitch.tv/oauth2/token")
        .form(&params)
        .send()
        .await
        .expect("Could not reach oauth token endpoint");
    let json_response = res.text().await.expect("Response was empty");
    println!("Json {}", json_response);
    let decoded_response: GetAccessTokenResponse =
        serde_json::from_str(json_response.as_str()).expect("Could not deserialize into Response");
    let user_access_token: UserAccessToken = UserAccessToken::from(decoded_response);

    let mut storage = CustomTokenStorage {
        location: config.credentials_file.clone(),
    };
    storage
        .update_token(&user_access_token)
        .await
        .expect("Error writing token");
}
