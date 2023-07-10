use crate::bot_config::BotConfig;
use crate::bot_token_storage::CustomTokenStorage;
use log::{debug, info};
use reqwest::Client;
use rocksdb::Direction::Forward;
use rocksdb::{IteratorMode, DB};
use serde::Deserialize;
use std::sync::Arc;
use tokio::sync::mpsc::Receiver;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use twitch_irc::login::TokenStorage;

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

fn process_command(cmd: Command) {
    use Command::*;
    match cmd {
        GetChannelStatus { key, resp } => {
            let db = DB::open_default("data/online.db").expect("Could not open online.db");
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
            let db = DB::open_default("data/online.db").expect("Could not open online.db");
            debug!("Setting channel {} online status: {}", key, val);
            let savable = if val { vec![1] } else { vec![0] };
            db.put(key, savable).expect("Cannot set online status");
            resp.send(()).expect("Cannot callback");
        }
        GetSoStatus {
            channel,
            so_channel,
            resp,
        } => {
            let db = DB::open_default("data/autoso.db").expect("Could not open autoso.db");
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
            let db = DB::open_default("data/autoso.db").expect("Could not open autoso.db");
            let key = format!("{} {}", channel, so_channel);
            let savable = if val { vec![1] } else { vec![0] };
            db.put(key, savable).expect("Cannot set online status");
            resp.send(()).expect("Cannot callback");
        }
        ResetSoStatus { channel, resp } => {
            let db = DB::open_default("data/autoso.db").expect("Could not open autoso.db");
            let it = db.iterator(IteratorMode::From(channel.as_ref(), Forward));
            for v in it {
                let (key, _) = v.expect("Error reading db");
                let key = String::from_utf8(key.into_vec()).expect("Could not parse key");
                if !key.starts_with(&channel) {
                    break;
                }
                db.put(key, vec![0]).expect("Could not set row to false ");
            }
            resp.send(()).expect("Cannot callback");
        }
    }
}

pub fn create_manager(conf: Arc<BotConfig>, mut receiver: Receiver<Command>) -> JoinHandle<()> {
    tokio::spawn(async move {
        let user_query = conf
            .channels
            .iter()
            .map(|c| c.channel_name.as_str())
            .map(|c| ("user_login", c))
            .collect::<Vec<_>>();
        info!("Starting manager");
        let db = DB::open_default("data/online.db").expect("Could not open online.db");
        conf.channels
            .iter()
            .map(|c| c.channel_name.as_str())
            .for_each(|channel| {
                db.put(channel, vec![0]).expect("Could not set db");
            });

        let http_client = Client::new();
        let mut storage = CustomTokenStorage {
            location: conf.credentials_file.clone(),
        };
        let res = http_client
            .get("https://api.twitch.tv/helix/streams")
            .bearer_auth(
                storage
                    .load_token()
                    .await
                    .expect("Error Reading the credential file")
                    .access_token
                    .as_str(),
            )
            .header("Client-Id", conf.client_id.as_str())
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
        info!("Starting to receive messages");
        while let Some(cmd) = receiver.recv().await {
            process_command(cmd);
        }
    })
}
