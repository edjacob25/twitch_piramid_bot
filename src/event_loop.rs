use crate::bot_config::BotConfig;
use crate::state_manager::Command;
use crate::twitch_ws::{GeneralMessage, MessageType, NotificationMessage, WelcomeMessage};
use futures_util::stream::SplitSink;
use futures_util::{SinkExt, StreamExt};
use log::{debug, error, info};
use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tokio::net::TcpStream;
use tokio::sync::mpsc::Sender;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tokio::time::sleep;
use tokio_tungstenite::tungstenite::{Message, WebSocket};
use tokio_tungstenite::{client_async, connect_async, MaybeTlsStream, WebSocketStream};

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

type WSClient = SplitSink<WebSocketStream<MaybeTlsStream<TcpStream>>, Message>;

async fn process_message(
    m: Message,
    broadcasters_ids: &Vec<String>,
    http_client: &Client,
    ws_client: &mut WSClient,
    token: &str,
    client_id: &str,
    sender: &Sender<Command>,
) {
    match m {
        Message::Text(msg) => {
            let msg: GeneralMessage =
                serde_json::from_str(&msg).expect("Could not parse message from ws");
            debug!("{:?}", msg);
            match msg.metadata.message_type {
                MessageType::Welcome => {
                    let m = WelcomeMessage::from(msg.payload);

                    for id in broadcasters_ids.iter() {
                        let res = http_client
                            .post("https://api.twitch.tv/helix/eventsub/subscriptions")
                            .bearer_auth(token.clone())
                            .header("Client-Id", client_id.clone())
                            .json(&RequestBody {
                                sub_type: "stream.online".to_string(),
                                version: "1".to_string(),
                                condition: Condition {
                                    broadcaster_user_id: id.clone(),
                                },
                                transport: Transport {
                                    method: "websocket".to_string(),
                                    session_id: m.session.id.clone(),
                                },
                            })
                            .send()
                            .await
                            .expect("Error sending event subscription request");

                        if res.status() == StatusCode::ACCEPTED {
                            info!("Accepted stream.online permission for {}", id)
                        } else {
                            error!("Error getting the stream.online sub")
                        }

                        let res = http_client
                            .post("https://api.twitch.tv/helix/eventsub/subscriptions")
                            .bearer_auth(token.clone())
                            .header("Client-Id", client_id.clone())
                            .json(&RequestBody {
                                sub_type: "stream.offline".to_string(),
                                version: "1".to_string(),
                                condition: Condition {
                                    broadcaster_user_id: id.clone(),
                                },
                                transport: Transport {
                                    method: "websocket".to_string(),
                                    session_id: m.session.id.clone(),
                                },
                            })
                            .send()
                            .await
                            .expect("Error sending event subscription request");

                        if res.status() == StatusCode::ACCEPTED {
                            info!("Accepted stream.offline permission for {}", id)
                        } else {
                            error!("Error getting the stream.offline sub")
                        }
                    }
                }
                MessageType::KeepAlive => {}
                MessageType::Notification => {
                    let m = NotificationMessage::from(msg.payload);
                    let (tx, rx) = oneshot::channel();
                    let val = m.event.online_type.is_some();

                    let cmd = Command::Set {
                        key: m.event.broadcaster_user_name,
                        val,
                        resp: tx,
                    };
                    let _ = sender.send(cmd).await;
                    assert_eq!(rx.await.unwrap(), ())
                }
                MessageType::Reconnect => {}
                MessageType::Revocation => {}
            }
        }
        Message::Close(_) => {}
        Message::Ping(data) => {
            let pong = Message::Pong(data);
            let _res = ws_client.send(pong);
        }
        _ => {}
    }
}

pub fn create_event_loop(conf: &BotConfig, sender: Sender<Command>) -> JoinHandle<()> {
    let token = conf.oauth_token.clone();
    let client_id = conf.client_id.clone();
    let user_query = conf
        .channels
        .iter()
        .map(|c| ("login".to_string(), c.channel_name.clone()))
        .collect::<Vec<_>>();

    tokio::spawn(async move {
        sleep(Duration::from_secs(30)).await;
        let (stream, _) = connect_async("wss://eventsub-beta.wss.twitch.tv/ws")
            .await
            .expect("Could not connect to twitch");
        let (mut ws_write, mut ws_read) = stream.split();
        let http_client = Client::new();

        let res = http_client
            .get("https://api.twitch.tv/helix/users")
            .bearer_auth(token.clone())
            .header("Client-Id", client_id.clone())
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
            .iter()
            .map(|p| p.id.clone())
            .collect::<Vec<_>>();

        while let Some(message) = ws_read.next().await {
            process_message(
                message.expect("Could not receive message from ws"),
                &broadcasters_ids,
                &http_client,
                &mut ws_write,
                &token,
                &client_id,
                &sender,
            )
            .await;
        }
    })
}
