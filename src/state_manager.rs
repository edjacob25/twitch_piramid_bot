use crate::bot_config::BotConfig;
use log::{debug, info};
use reqwest::Client;
use rocksdb::Direction::Forward;
use rocksdb::{IteratorMode, DB};
use serde::Deserialize;
use tokio::sync::mpsc::Receiver;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

type Responder<T> = oneshot::Sender<T>;

#[derive(Debug)]
pub enum Command {
    GetChannelStatus {
        key: String,
        resp: Responder<bool>,
    },
    SetChannelStatus {
        key: String,
        val: bool,
        resp: Responder<()>,
    },
    GetSoStatus {
        channel: String,
        so_channel: String,
        resp: Responder<bool>,
    },
    SetSoStatus {
        channel: String,
        so_channel: String,
        val: bool,
        resp: Responder<()>,
    },
    ResetSoStatus {
        channel: String,
        resp: Responder<()>,
    },
}

#[derive(Debug, Deserialize)]
struct ChannelsResponse {
    data: Vec<ChannelData>,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
struct ChannelData {
    user_login: String,
}

pub fn create_manager(conf: &BotConfig, mut receiver: Receiver<Command>) -> JoinHandle<()> {
    let channel_names = conf
        .channels
        .iter()
        .map(|c| c.channel_name.clone())
        .collect::<Vec<_>>();

    let token = conf.oauth_token.clone();
    let client_id = conf.client_id.clone();
    let user_query = channel_names
        .iter()
        .map(|c| ("user_login".to_string(), c.clone()))
        .collect::<Vec<_>>();

    let handle = tokio::spawn(async move {
        info!("Starting manager");
        let db = DB::open_default("online.db").unwrap();
        channel_names.iter().for_each(|channel| {
            db.put(channel.clone(), vec![0]).expect("Could not set db");
        });

        let http_client = Client::new();

        let res = http_client
            .get("https://api.twitch.tv/helix/streams")
            .bearer_auth(token.clone())
            .header("Client-Id", client_id.clone())
            .query(&user_query)
            .send()
            .await
            .expect("Could not reach twitch api users")
            .text()
            .await
            .expect("Could not parse response");

        let online_channels: ChannelsResponse =
            serde_json::from_str(&res).expect("Could not parse channel data");
        for c in online_channels.data {
            db.put(c.user_login, vec![1])
                .expect("Could not put online channels");
        }
        drop(db);
        while let Some(cmd) = receiver.recv().await {
            use Command::*;
            match cmd {
                GetChannelStatus { key, resp } => {
                    let db = DB::open_default("online.db").unwrap();
                    let res = match db.get(&key) {
                        Ok(Some(values)) => {
                            let val = *values.first().expect("No bytes retrieved");
                            val > 0
                        }
                        Ok(None) => false,
                        Err(_) => false,
                    };
                    debug!("Channel {} online status: {}", key, res);
                    let _ = resp.send(res);
                }
                SetChannelStatus { key, val, resp } => {
                    let db = DB::open_default("online.db").unwrap();
                    debug!("Setting channel {} online status: {}", key, res);
                    let savable = if val { vec![1] } else { vec![0] };
                    db.put(key, savable).expect("Cannot set online status");
                    resp.send(()).expect("Cannot callback");
                }
                GetSoStatus {
                    channel,
                    so_channel,
                    resp,
                } => {
                    let db = DB::open_default("autoso.db").unwrap();
                    let key = format!("{} {}", channel, so_channel);
                    let res = match db.get(&key) {
                        Ok(Some(values)) => {
                            let val = *values.first().expect("No bytes retrieved");
                            val > 0
                        }
                        Ok(None) => {
                            db.put(&key, vec![0]).expect("Could not create db row");
                            false
                        }
                        Err(_) => false,
                    };
                    debug!(
                        "In channel {}, so_channel {} has status: {}",
                        channel, so_channel, res
                    );
                    let _ = resp.send(res);
                }
                SetSoStatus {
                    channel,
                    so_channel,
                    val,
                    resp,
                } => {
                    let db = DB::open_default("autoso.db").unwrap();
                    let key = format!("{} {}", channel, so_channel);
                    let savable = if val { vec![1] } else { vec![0] };
                    db.put(key, savable).expect("Cannot set online status");
                    resp.send(()).expect("Cannot callback");
                }
                ResetSoStatus { channel, resp } => {
                    let db = DB::open_default("autoso.db").unwrap();
                    let it = db.iterator(IteratorMode::From(channel.as_ref(), Forward));
                    for v in it {
                        let (key, _) = v.expect("Error reading db");
                        let key = String::from_utf8(key.into_vec()).unwrap();
                        if !key.starts_with(&channel) {
                            break;
                        }
                        db.put(key, vec![0]).expect("Could not set row to false ");
                    }
                    resp.send(()).expect("Cannot callback");
                }
            }
        }
    });
    handle
}
