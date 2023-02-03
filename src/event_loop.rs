use crate::bot_config::BotConfig;
use crate::event_loop::MessageResponse::Continue;
use crate::state_manager::Command;
use crate::twitch_ws::*;
use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt};
use log::{debug, error, info, warn};
use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::net::TcpStream;
use tokio::sync::mpsc::Sender;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{connect_async, MaybeTlsStream, WebSocketStream};

#[derive(Debug, Serialize)]
struct RequestBody {
    #[serde(rename = "type")]
    sub_type: String,
    version: String,
    condition: Condition,
    transport: Transport,
}

#[derive(Debug, Serialize)]
struct Condition {
    broadcaster_user_id: String,
}
#[derive(Debug, Serialize)]
struct Transport {
    method: String,
    session_id: String,
}

#[derive(Debug, Deserialize)]
struct UsersResponse {
    data: Vec<UserData>,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
struct UserData {
    id: String,
    login: String,
    display_name: String,
    #[serde(rename = "type")]
    user_type: String,
    broadcaster_type: String,
    description: String,
    profile_image_url: String,
    offline_image_url: String,
    view_count: u32,
    created_at: String,
}

type WSWriter = SplitSink<WebSocketStream<MaybeTlsStream<TcpStream>>, Message>;
type WSReader = SplitStream<WebSocketStream<MaybeTlsStream<TcpStream>>>;

enum MessageResponse {
    ConnectionSucessful,
    Continue,
    Reconnect(String),
}

struct EventData<'a> {
    event: &'a str,
    broadcaster_id: &'a str,
    session_id: &'a str,
}

struct HttpHeaders<'a> {
    token: &'a str,
    client_id: &'a str,
}

async fn register_event(
    event_data: EventData<'_>,
    http_client: &Client,
    headers: &HttpHeaders<'_>,
) {
    let res = http_client
        .post("https://api.twitch.tv/helix/eventsub/subscriptions")
        .bearer_auth(headers.token)
        .header("Client-Id", headers.client_id)
        .json(&RequestBody {
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
            event_data.event, event_data.broadcaster_id
        )
    } else {
        error!("Error getting the {} sub", event_data.event)
    }
}

async fn process_text_message(
    broadcasters_ids: &Vec<String>,
    http_client: &Client,
    headers: &HttpHeaders<'_>,
    sender: &Sender<Command>,
    reconnecting: bool,
    msg: &String,
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

            for id in broadcasters_ids.iter() {
                let event_data = EventData {
                    event: "stream.online",
                    broadcaster_id: id,
                    session_id: &m.session.id,
                };
                register_event(event_data, http_client, headers).await;

                let event_data = EventData {
                    event: "stream.offline",
                    broadcaster_id: id,
                    session_id: &m.session.id,
                };
                register_event(event_data, http_client, headers).await;

                let event_data = EventData {
                    event: "channel.update",
                    broadcaster_id: id,
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
                let msg = format!("Stream {} is changing info, title is {} ", m.event.broadcaster_user_name, m.event.title.unwrap());
                info!("{}", msg);
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
                assert_eq!(rx.await.unwrap(), ())
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
    return Continue;
}

async fn process_message(
    m: Message,
    broadcasters_ids: &Vec<String>,
    http_client: &Client,
    ws_client: &mut WSWriter,
    headers: &HttpHeaders<'_>,
    sender: &Sender<Command>,
    reconnecting: bool,
) -> MessageResponse {
    match m {
        Message::Text(msg) => {
            return process_text_message(
                broadcasters_ids,
                http_client,
                headers,
                sender,
                reconnecting,
                &msg,
            )
            .await;
        }
        Message::Close(_) => {
            error!("Closing connection");
        }
        Message::Ping(data) => {
            let pong = Message::Pong(data);
            let _res = ws_client.send(pong);
        }
        _ => {}
    }
    return Continue;
}

pub fn create_event_loop(conf: Arc<BotConfig>, sender: Sender<Command>) -> JoinHandle<()> {
    tokio::spawn(async move {
        let headers = HttpHeaders {
            token: conf.oauth_token.as_str(),
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
            .bearer_auth(headers.token)
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
            .map(|p| p.id)
            .collect::<Vec<_>>();

        let mut address = "wss://eventsub-beta.wss.twitch.tv/ws".to_string();
        let mut last_client: Option<(WSWriter, WSReader)> = None;
        loop {
            let (stream, _) = connect_async(&address)
                .await
                .expect("Could not connect to twitch");
            info!("Connected to {}", address);
            let (mut ws_write, mut ws_read) = stream.split();
            while let Some(message) = ws_read.next().await {
                let rec = process_message(
                    message.expect("Could not receive message from ws"),
                    &broadcasters_ids,
                    &http_client,
                    &mut ws_write,
                    &headers,
                    &sender,
                    last_client.is_some(),
                )
                .await;
                match rec {
                    MessageResponse::ConnectionSucessful if last_client.is_some() => {
                        warn!("Actually closing the connection");
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
                    _ => {}
                }
            }
            last_client = Some((ws_write, ws_read));
        }
    })
}
