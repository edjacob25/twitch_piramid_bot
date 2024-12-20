use crate::bot_config::{BotConfig, ChannelConfig, Ntfy};
use crate::bot_token_storage::CustomTokenStorage;
use crate::event_data::*;
use crate::state_manager::Command;
use crate::twitch_ws::*;
use anyhow::{anyhow, bail, Result};
use futures_util::{SinkExt, StreamExt};
use log::{debug, error, info, warn};
use reqwest::{Client, StatusCode};
use std::collections::HashMap;
use std::ops::Add;
use std::sync::Arc;
use tokio::sync::mpsc::Sender;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::error::Error as WsError;
use tokio_tungstenite::tungstenite::Message;
use twitch_irc::login::{LoginCredentials, RefreshingLoginCredentials, TokenStorage};

struct EventLoop {
    conf: Arc<BotConfig>,
    sender: Sender<Command>,
    http_client: Client,
    headers: HttpHeaders,
    broadcasters_ids: Vec<(String, String)>,
}

impl EventLoop {
    pub async fn new(conf: Arc<BotConfig>, sender: Sender<Command>) -> Self {
        let http_client = Client::new();
        let mut storage = CustomTokenStorage {
            location: conf.credentials_file.clone(),
        };
        let token = storage
            .load_token()
            .await
            .expect("Error Reading the credential file")
            .access_token;
        let headers = HttpHeaders {
            token,
            client_id: conf.client_id.clone(),
        };

        let user_query = conf
            .channels
            .iter()
            .map(|c| ("login", c.channel_name.as_str()))
            .collect::<Vec<_>>();

        let res = http_client
            .get("https://api.twitch.tv/helix/users")
            .bearer_auth(headers.token.as_str())
            .header("Client-Id", headers.client_id.as_str())
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

        Self {
            conf,
            sender,
            http_client,
            headers,
            broadcasters_ids,
        }
    }

    async fn register_event(&self, event_data: &EventData<'_>) -> Result<String> {
        let res = self
            .http_client
            .post("https://api.twitch.tv/helix/eventsub/subscriptions")
            .bearer_auth(self.headers.token.as_str())
            .header("Client-Id", &self.headers.client_id)
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
            .await?;

        if res.status() == StatusCode::ACCEPTED {
            info!(
                "Accepted {} permission for {}",
                event_data.event, event_data.name
            );
            let num = res.json::<serde_json::Value>().await?["data"]["id"]
                .as_str()
                .ok_or_else(|| anyhow!("Could not get subscription id"))?
                .to_string();

            Ok(num)
        } else {
            error!(
                "Error getting the {} sub for channel {}: Code: {} - {}",
                event_data.event,
                event_data.name,
                res.status(),
                res.text().await.unwrap_or("Unknown error".to_string())
            );
            bail!(
                "Error getting the {} sub for channel {}",
                event_data.event,
                event_data.name
            );
        }
    }

    async fn unregister_events(&self, id: &str) -> Result<()> {
        let params = [("id", id)];
        let url = reqwest::Url::parse_with_params(
            "https://api.twitch.tv/helix/eventsub/subscriptions",
            params,
        )?;
        let _res = self
            .http_client
            .delete(url)
            .bearer_auth(self.headers.token.as_str())
            .header("Client-Id", &self.headers.client_id)
            .send()
            .await?;
        Ok(())
    }

    async fn handle_event(&self, m: NotificationMessage) {
        match m.subscription.sub_type {
            EventType::Online => {
                let (tx, rx) = oneshot::channel();
                info!(
                    "Resetting autoso status for channel {}",
                    m.event.broadcaster_user_name
                );
                let cmd = Command::ResetSoStatus {
                    channel: m.event.broadcaster_user_name.clone(),
                    resp: tx,
                };
                let _ = self.sender.send(cmd).await;
                assert_eq!(rx.await.unwrap(), ());

                send_notification(
                    &self.conf.ntfy,
                    "Stream starting".to_string(),
                    Some(&m.event.broadcaster_user_name),
                )
                .await;
                info!("Sending true to channel {}", m.event.broadcaster_user_name);
                let (tx, rx) = oneshot::channel();
                let cmd = Command::SetChannelStatus {
                    key: m.event.broadcaster_user_name,
                    val: true,
                    resp: tx,
                };
                let _ = self.sender.send(cmd).await;
                assert_eq!(rx.await.unwrap(), ());
            }
            EventType::Offline => {
                info!("Sending false to channel {}", m.event.broadcaster_user_name);
                let (tx, rx) = oneshot::channel();
                let cmd = Command::SetChannelStatus {
                    key: m.event.broadcaster_user_name,
                    val: false,
                    resp: tx,
                };
                let _ = self.sender.send(cmd).await;
                assert_eq!(rx.await.unwrap(), ());
            }
            EventType::StreamChange => {
                let (tx, rx) = oneshot::channel();
                let cmd = Command::GetStreamInfo {
                    channel: m.event.broadcaster_user_name.clone(),
                    resp: tx,
                };
                self.sender
                    .send(cmd)
                    .await
                    .expect("Could not send request for channel status");
                let event = match rx.await {
                    Ok(res) => res,
                    Err(_) => panic!("wot m8"),
                };

                let mut msg = format!(
                    "Stream {} is changing info: ",
                    m.event.broadcaster_user_name.clone()
                );
                let new_title = m.event.title.clone().unwrap();
                if event.title.unwrap_or("".to_string()) != new_title {
                    msg = msg.add(&format!("Title -> {}, ", new_title))
                }

                let new_cat = m.event.category_name.clone().unwrap();
                if event.category_name.unwrap_or("".to_string()) != new_cat {
                    msg = msg.add(&format!("Category -> {}", new_cat))
                }

                info!("{}", msg);
                send_notification(
                    &self.conf.ntfy,
                    msg,
                    Some(&m.event.broadcaster_user_name.clone()),
                )
                .await;

                let (tx, rx) = oneshot::channel();
                let cmd = Command::SetStreamInfo {
                    channel: m.event.broadcaster_user_name.clone(),
                    event: m.event,
                    resp: tx,
                };
                let _ = self.sender.send(cmd).await;
                assert_eq!(rx.await.unwrap(), ());
            }
            EventType::PredictionStart => {
                let question = m
                    .event
                    .title
                    .unwrap_or("Could not get question".to_string());
                info!("Starting prediction for {}", m.event.broadcaster_user_name);
                let (tx, rx) = oneshot::channel();
                let cmd = Command::StartPrediction {
                    channel: m.event.broadcaster_user_name,
                    question,
                    resp: tx,
                };
                let _ = self.sender.send(cmd).await;
                assert_eq!(rx.await.unwrap(), ());
            }
            EventType::PredictionProgress => {
                let options = m.event.outcomes.expect("Poll options have to exist");

                let responses = options
                    .iter()
                    .map(|option| {
                        (
                            option.title.clone(),
                            option
                                .top_predictors
                                .iter()
                                .map(|p| (p.user_name.clone(), p.channel_points_used))
                                .collect::<Vec<(String, u32)>>(),
                        )
                    })
                    .collect::<Vec<(String, Vec<(String, u32)>)>>();
                info!("Prediction progress for {}", m.event.broadcaster_user_name);
                let (tx, rx) = oneshot::channel();
                let cmd = Command::PredictionProgress {
                    channel: m.event.broadcaster_user_name,
                    responses,
                    resp: tx,
                };
                let _ = self.sender.send(cmd).await;
                assert_eq!(rx.await.unwrap(), ());
            }
            EventType::PredictionEnd => {
                info!("Ending prediction for {}", m.event.broadcaster_user_name);
                let (tx, rx) = oneshot::channel();
                let cmd = Command::PredictionEnd {
                    channel: m.event.broadcaster_user_name,
                    resp: tx,
                };
                let _ = self.sender.send(cmd).await;
                assert_eq!(rx.await.unwrap(), ());
            }
            EventType::Other => {}
        }
    }

    async fn refresh_credentials(&mut self) {
        let credentials = RefreshingLoginCredentials::init(
            self.conf.client_id.clone(),
            self.conf.client_secret.clone(),
            CustomTokenStorage {
                location: self.conf.credentials_file.clone().clone(),
            },
        );

        let c = credentials.get_credentials().await;
        self.headers.token = c.unwrap().token.unwrap();
    }

    async fn process_text_message(&self, msg: &String, reconnecting: bool) -> MessageResponse {
        let msg: GeneralMessage =
            serde_json::from_str(&msg).expect("Could not parse message from ws");
        debug!("{:?}", msg);
        match msg.metadata.message_type {
            MessageType::Welcome => {
                if reconnecting {
                    warn!("Reconnection successful");
                    return MessageResponse::ConnectionSuccessful(Vec::new());
                }

                let m = WelcomeMessage::from(msg.payload);
                let channel_configs: HashMap<String, &ChannelConfig> = HashMap::from_iter(
                    self.conf
                        .channels
                        .iter()
                        .map(|c| (c.channel_name.clone(), c)),
                );
                let mut subscription_ids = Vec::new();
                for (id, name) in self.broadcasters_ids.iter() {
                    let mut event_data = EventData {
                        event: "stream.online",
                        broadcaster_id: id,
                        name,
                        session_id: &m.session.id,
                    };
                    if let Ok(num) = self.register_event(&event_data).await {
                        subscription_ids.push(num)
                    };

                    event_data.event = "stream.offline";
                    if let Ok(num) = self.register_event(&event_data).await {
                        subscription_ids.push(num)
                    };

                    event_data.event = "channel.update";
                    if let Ok(num) = self.register_event(&event_data).await {
                        subscription_ids.push(num)
                    };

                    if channel_configs.contains_key(name)
                        && channel_configs
                            .get(name)
                            .unwrap()
                            .prediction_monitoring
                            .unwrap_or(false)
                    {
                        event_data.event = "channel.prediction.begin";
                        if let Ok(num) = self.register_event(&event_data).await {
                            subscription_ids.push(num)
                        };
                        event_data.event = "channel.prediction.progress";
                        if let Ok(num) = self.register_event(&event_data).await {
                            subscription_ids.push(num)
                        };
                        event_data.event = "channel.prediction.end";
                        if let Ok(num) = self.register_event(&event_data).await {
                            subscription_ids.push(num)
                        };
                    }
                }

                return MessageResponse::ConnectionSuccessful(subscription_ids);
            }
            MessageType::KeepAlive => {}
            MessageType::Notification => {
                let m = NotificationMessage::from(msg);
                self.handle_event(m).await;
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
        MessageResponse::Continue
    }

    async fn process_message(&self, m: Message, reconnecting: bool) -> MessageResponse {
        match m {
            Message::Text(msg) => {
                return self.process_text_message(&msg, reconnecting).await;
            }
            Message::Close(close) => {
                match close {
                    None => {}
                    Some(c) => {
                        error!("Closing reason {}", c);
                        send_notification(
                            &self.conf.ntfy,
                            format!("Closing connection: {:?}", c),
                            None,
                        )
                        .await;
                        if c.reason.contains("4007") {
                            warn!("WTF");
                        }
                    }
                }
                error!("Closing connection");
                return MessageResponse::Close;
            }
            Message::Ping(data) => {
                return MessageResponse::Pong(data);
            }
            _ => {}
        }
        MessageResponse::Continue
    }

    pub async fn run(&mut self) {
        let mut address = "wss://eventsub.wss.twitch.tv/ws".to_string();
        let mut last_client: Option<(WSWriter, WSReader)> = None;
        let mut current_subscriptions = Vec::new();
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
                            &self.conf.ntfy,
                            format!("Could not receive message with error: {:?}", e),
                            None,
                        )
                        .await;
                        error!("Could not receive message from ws: {:?}", e);
                        if let WsError::Io(_) = e {
                            error!("Connection reset route");
                            Message::Close(None)
                        } else {
                            error!("Ignoring the error");
                            continue;
                        }
                    }
                };

                let reconnecting = last_client.is_some();

                let response = self.process_message(m, reconnecting).await;
                match response {
                    MessageResponse::ConnectionSuccessful(_ids) if reconnecting => {
                        warn!("Actually closing the old connection");
                        let (mut old_w, old_r) = last_client.unwrap();
                        let _ = old_w.close();
                        last_client = None;
                        drop(old_w);
                        drop(old_r);
                    }
                    MessageResponse::ConnectionSuccessful(ids) => current_subscriptions = ids,
                    MessageResponse::Reconnect(reconnect_url) => {
                        address = reconnect_url;
                        break;
                    }
                    MessageResponse::Close => {
                        warn!("Resetting connection to base level");
                        address = "wss://eventsub.wss.twitch.tv/ws".to_string();
                        self.refresh_credentials().await;
                        let _ = ws_write.close();
                        // if last_client.is_some() {
                        //     let (mut old_w, old_r) = last_client.unwrap();
                        //     let _ = old_w.close();
                        //     last_client = None;
                        //     drop(old_w);
                        //     drop(old_r);
                        //     error!("Closing it and dropping if necessary")
                        // }
                        for current_subscription in current_subscriptions.iter() {
                            if let Err(e) = self.unregister_events(current_subscription).await {
                                error!(
                                    "Could not unregister event {} with error {:?}",
                                    current_subscription, e
                                );
                            }
                        }
                        current_subscriptions.clear();
                        continue 'ws_creation;
                    }
                    MessageResponse::Pong(data) => {
                        let pong = Message::Pong(data);
                        let _res = ws_write.send(pong);
                    }
                    _ => {}
                }
            }
            last_client = Some((ws_write, ws_read));
        }
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
            if let Err(e) = req.send().await {
                error!("Error sending notification to ntfy: {}", e);
            }
        });
    }
}

pub fn create_event_loop(conf: Arc<BotConfig>, sender: Sender<Command>) -> JoinHandle<()> {
    tokio::spawn(async move {
        EventLoop::new(conf, sender).await.run().await;
    })
}
