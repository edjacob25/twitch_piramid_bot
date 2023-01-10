use crate::bot_config::BotConfig;
use crate::state_manager::Command;
use crate::twitch_ws::{GeneralMessage, MessageType, NotificationMessage, WelcomeMessage};
use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc::Sender;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use websocket::stream::sync::NetworkStream;
use websocket::{ClientBuilder, OwnedMessage};

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

type WSClient = websocket::client::sync::Client<Box<dyn NetworkStream + Send>>;

async fn process_message(
    m: OwnedMessage,
    broadcasters_ids: &Vec<String>,
    http_client: &Client,
    ws_client: &mut WSClient,
    token: &str,
    client_id: &str,
    sender: &Sender<Command>,
) {
    match m {
        OwnedMessage::Text(msg) => {
            let msg: GeneralMessage = serde_json::from_str(&msg).unwrap();
            println!("{:?}", msg);
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
                            println!("Accepted permission")
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
                            println!("Accepted permission")
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
        OwnedMessage::Close(_) => {}
        OwnedMessage::Ping(data) => {
            let pong = OwnedMessage::Pong(data);
            let _res = ws_client.send_message(&pong);
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
        let mut builder = ClientBuilder::new("wss://eventsub-beta.wss.twitch.tv/ws")
            .expect("Cannot create builder");
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

        let mut ws_client = builder.connect(None).unwrap();

        while let Some(message) = ws_client.incoming_messages().next() {
            process_message(
                message.unwrap(),
                &broadcasters_ids,
                &http_client,
                &mut ws_client,
                &token,
                &client_id,
                &sender,
            )
            .await;
        }
    })
}
