use crate::bot_config::BotConfig;
use log::{debug, info};
use reqwest::Client;
use rocksdb::DB;
use serde::Deserialize;
use tokio::sync::mpsc::Receiver;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

type Responder<T> = oneshot::Sender<T>;

#[derive(Debug)]
pub enum Command {
    Get {
        key: String,
        resp: Responder<bool>,
    },
    Set {
        key: String,
        val: bool,
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

        while let Some(cmd) = receiver.recv().await {
            use Command::*;
            match cmd {
                Get { key, resp } => {
                    let res = match db.get(key.clone()) {
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
                Set { key, val, resp } => {
                    debug!("Setting channel {} online status: {}", key, res);
                    let savable = if val { vec![1] } else { vec![0] };
                    db.put(key, savable).expect("Cannot set online status");
                    resp.send(()).expect("Cannot callback");
                }
            }
        }
    });
    handle
}
