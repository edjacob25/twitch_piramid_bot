use crate::bot_config::{BotConfig, Ntfy};
use crate::bot_token_storage::CustomTokenStorage;
use crate::event_loop::MessageResponse::Continue;
use crate::event_data::*;
use crate::state_manager::Command;
use crate::twitch_ws::*;
use futures_util::{SinkExt, StreamExt};
use log::{debug, error, info, warn};
use reqwest::{Client, StatusCode};
use std::sync::Arc;
use tokio::sync::mpsc::Sender;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;
use twitch_irc::login::{LoginCredentials, RefreshingLoginCredentials, TokenStorage};

async fn register_event(
    event_data: EventData<'_>,
    http_client: &Client,
    headers: &HttpHeaders<'_>,
) {
    let res = http_client
        .post("https://api.twitch.tv/helix/eventsub/subscriptions")
        .bearer_auth(headers.token.as_str())
        .header("Client-Id", headers.client_id)
        .json(&RequestBody {
        .json(&EventRequestBody {
            sub_type: event_data.event.to_string(),
            version: "1".to_string(),
            condition: Condition {
                broadcaster_user_id: event_data.broadcaster_id.to_string(),
            },
            transport: Transport {
                method: "websocket".to_string(),
                session_id: event_data.session_id.to_string(),
            },
        })
        .send()
        .await
        .expect("Error sending event subscription request");

    if res.status() == StatusCode::ACCEPTED {
        info!(
            "Accepted {} permission for {}",
            event_data.event, event_data.name
        )
    } else {
        error!(
            "Error getting the {} sub for channel {}: Code: {} - {}",
            event_data.event,
            event_data.name,
            res.status(),
            res.text().await.unwrap_or("Unknown error".to_string())
        )
    }
}

async fn send_notification(ntfy: &Option<Ntfy>, msg: String, login: Option<&str>) {
    if let Some(nt) = ntfy.clone() {
        let address = if let Some(log) = login {
            Some(format!("https://www.twitch.tv/{}", log))
        } else {
            None
        };
        tokio::spawn(async move {
            let cl = Client::new();
            let mut req = cl
                .post(nt.address)
                .basic_auth(nt.user, Some(nt.pass))
                .body(msg);
            if let Some(adr) = address {
                req = req.header("Click", adr);
            }
            let _res = req.send().await.expect("Could not reach ntfy server");
        });
    }
}

async fn process_text_message(
    msg: &String,
    broadcasters_ids: &Vec<(String, String)>,
    http_client: &Client,
    headers: &HttpHeaders<'_>,
    sender: &Sender<Command>,
    reconnecting: bool,
    ntfy: &Option<Ntfy>,
) -> MessageResponse {
    let msg: GeneralMessage = serde_json::from_str(&msg).expect("Could not parse message from ws");
    debug!("{:?}", msg);
    match msg.metadata.message_type {
        MessageType::Welcome => {
            if reconnecting {
                warn!("Reconnection successful");
                return MessageResponse::ConnectionSucessful;
            }

            let m = WelcomeMessage::from(msg.payload);

            for (id, name) in broadcasters_ids.iter() {
                let event_data = EventData {
                    event: "stream.online",
                    broadcaster_id: id,
                    name,
                    session_id: &m.session.id,
                };
                register_event(event_data, http_client, headers).await;

                let event_data = EventData {
                    event: "stream.offline",
                    broadcaster_id: id,
                    name,
                    session_id: &m.session.id,
                };
                register_event(event_data, http_client, headers).await;

                let event_data = EventData {
                    event: "channel.update",
                    broadcaster_id: id,
                    name,
                    session_id: &m.session.id,
                };
                register_event(event_data, http_client, headers).await;
            }

            return MessageResponse::ConnectionSucessful;
        }
        MessageType::KeepAlive => {}
        MessageType::Notification => {
            let m = NotificationMessage::from(msg.payload);

            if m.event.title.is_some() {
                let msg = format!(
                    "Stream {} is changing info, title is {} ",
                    m.event.broadcaster_user_name,
                    m.event.title.unwrap()
                );
                info!("{}", msg);
                send_notification(ntfy, msg, Some(&m.event.broadcaster_user_name)).await;
                return Continue;
            }

            let starting_stream = m.event.online_type.is_some();

            if starting_stream {
                let (tx, rx) = oneshot::channel();
                info!(
                    "Resetting autoso status for channel {}",
                    m.event.broadcaster_user_name
                );
                let cmd = Command::ResetSoStatus {
                    channel: m.event.broadcaster_user_name.clone(),
                    resp: tx,
                };
                let _ = sender.send(cmd).await;
                assert_eq!(rx.await.unwrap(), ());

                send_notification(
                    ntfy,
                    "Stream starting".to_string(),
                    Some(&m.event.broadcaster_user_name),
                )
                .await;
            }

            info!(
                "Sending {} to channel {}",
                starting_stream, m.event.broadcaster_user_name,
            );
            let (tx, rx) = oneshot::channel();
            let cmd = Command::SetChannelStatus {
                key: m.event.broadcaster_user_name,
                val: starting_stream,
                resp: tx,
            };
            let _ = sender.send(cmd).await;
            assert_eq!(rx.await.unwrap(), ());
        }
        MessageType::Reconnect => {
            let m = ReconnectMessage::from(msg.payload);
            error!("Should reconnect");
            return MessageResponse::Reconnect(
                m.session.reconnect_url.expect("There was no reconnect url"),
            );
        }
        MessageType::Revocation => {}
    }
    return MessageResponse::Continue;
}

async fn process_message(
    m: Message,
    broadcasters_ids: &Vec<(String, String)>,
    http_client: &Client,
    ws_client: &mut WSWriter,
    headers: &HttpHeaders<'_>,
    sender: &Sender<Command>,
    reconnecting: bool,
    ntfy: &Option<Ntfy>,
) -> MessageResponse {
    match m {
        Message::Text(msg) => {
            return process_text_message(
                &msg,
                broadcasters_ids,
                http_client,
                headers,
                sender,
                reconnecting,
                ntfy,
            )
            .await;
        }
        Message::Close(close) => {
            match close {
                None => {}
                Some(c) => {
                    error!("Closing reason {}", c);
                    send_notification(ntfy, format!("Closing connection: {:?}", c), None).await;
                    if c.reason.contains("4007") {
                        warn!("WTF");
                    }
                }
            }
            let _ = ws_client.close();
            error!("Closing connection");
            return MessageResponse::Close;
        }
        Message::Ping(data) => {
            let pong = Message::Pong(data);
            let _res = ws_client.send(pong);
        }
        _ => {}
    }
    return MessageResponse::Continue;
}

pub fn create_event_loop(conf: Arc<BotConfig>, sender: Sender<Command>) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut storage = CustomTokenStorage {
            location: conf.credentials_file.clone(),
        };
        let token = storage
            .load_token()
            .await
            .expect("Error Reading the credential file")
            .access_token;
        let mut headers = HttpHeaders {
            token,
            client_id: conf.client_id.as_str(),
        };
        let user_query = conf
            .channels
            .iter()
            .map(|c| ("login", c.channel_name.as_str()))
            .collect::<Vec<_>>();

        let http_client = Client::new();
        let res = http_client
            .get("https://api.twitch.tv/helix/users")
            .bearer_auth(headers.token.as_str())
            .header("Client-Id", headers.client_id)
            .query(&user_query)
            .send()
            .await
            .expect("Could not reach twitch api users")
            .text()
            .await
            .expect("Could not parse twitch user data");

        let persons_data: UsersResponse =
            serde_json::from_str(&res).expect("Could not parse persons");
        let broadcasters_ids = persons_data
            .data
            .into_iter()
            .map(|p| (p.id, p.display_name))
            .collect::<Vec<_>>();

        let mut address = "wss://eventsub.wss.twitch.tv/ws".to_string();
        let mut last_client: Option<(WSWriter, WSReader)> = None;
        'ws_creation: loop {
            let (stream, _) = connect_async(&address)
                .await
                .expect("Could not connect to twitch");
            info!("Connected to {}", address);
            let (mut ws_write, mut ws_read) = stream.split();
            while let Some(message) = ws_read.next().await {
                let m = match message {
                    Ok(m) => m,
                    Err(e) => {
                        send_notification(
                            &conf.ntfy,
                            format!("Could not receive message with error: {:?}", e),
                            None,
                        )
                        .await;
                        error!("Could not receive message from ws: {:?}", e);
                        continue;
                    }
                };

                let rec = process_message(
                    m,
                    &broadcasters_ids,
                    &http_client,
                    &mut ws_write,
                    &headers,
                    &sender,
                    last_client.is_some(),
                    &conf.ntfy,
                )
                .await;
                match rec {
                    MessageResponse::ConnectionSucessful if last_client.is_some() => {
                        warn!("Actually closing the old connection");
                        let (mut old_w, old_r) = last_client.unwrap();
                        let _ = old_w.close();
                        last_client = None;
                        drop(old_w);
                        drop(old_r);
                    }
                    MessageResponse::Reconnect(reconnect_url) => {
                        address = reconnect_url;
                        break;
                    }
                    MessageResponse::Close => {
                        warn!("Resetting connection to base level");
                        address = "wss://eventsub.wss.twitch.tv/ws".to_string();
                        let credentials = RefreshingLoginCredentials::init(
                            conf.client_id.clone(),
                            conf.client_secret.clone(),
                            CustomTokenStorage {
                                location: storage.location.clone(),
                            },
                        );

                        let c = credentials.get_credentials().await;
                        headers.token = c.unwrap().token.unwrap();
                        if last_client.is_some() {
                            let (mut old_w, old_r) = last_client.unwrap();
                            let _ = old_w.close();
                            last_client = None;
                            drop(old_w);
                            drop(old_r);
                        }
                        continue 'ws_creation;
                    }
                    _ => {}
                }
            }
            last_client = Some((ws_write, ws_read));
        }
    })
}
