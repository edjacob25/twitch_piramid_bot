use crate::bot_config::BotConfig;
use crate::bot_token_storage::CustomTokenStorage;
use crate::twitch_ws::Event;
use anyhow::{Context, Result};
use chrono::{DateTime, Local, Utc};
use log::{debug, error, info};
use reqwest::Client;
use rocksdb::Direction::Forward;
use rocksdb::{IteratorMode, DB};
use rusqlite::Connection;
use serde::Deserialize;
use std::collections::HashMap;
use std::fs;
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
    },
    ResetSoStatus {
        channel: String,
    },
    StartPrediction {
        channel: String,
        question: String,
    },
    PredictionProgress {
        channel: String,
        responses: Vec<(String, Vec<(String, u32)>)>,
    },
    PredictionEnd {
        channel: String,
    },
    GetStreamInfo {
        channel: String,
        resp: Responder<Event>,
    },
    SetStreamInfo {
        channel: String,
        event: Box<Event>,
    },
    CountBits {
        channel: String,
        user: String,
        bits: u64,
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

struct StreamPrediction {
    name: String,
    date: DateTime<Utc>,
    predictions: HashMap<String, HashMap<String, u32>>,
}

fn process_command(cmd: Command, streams_data: &mut HashMap<String, Event>) {
    let mut predictions: HashMap<String, StreamPrediction> = HashMap::new();
    use Command::*;
    match cmd {
        GetChannelStatus { key, resp } => {
            let db = DB::open_default("data/online.db").expect("Could not open online.db");
            let res = match db.get(&key) {
                Ok(Some(values)) => {
                    let val = *values.first().expect("No bytes retrieved");
                    val > 0
                }
                Ok(None) | Err(_) => false,
            };
            debug!("Channel {} online status: {}", key, res);
            let _ = resp.send(res);
        }
        SetChannelStatus { key, val } => {
            let db = DB::open_default("data/online.db").expect("Could not open online.db");
            debug!("Setting channel {} online status: {}", key, val);
            let savable = if val { vec![1] } else { vec![0] };
            db.put(key, savable).expect("Cannot set online status");
        }
        GetSoStatus {
            channel,
            so_channel,
            resp,
        } => {
            let db = DB::open_default("data/autoso.db").expect("Could not open autoso.db");
            let key = format!("{channel} {so_channel}");
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
            debug!("In channel {}, so_channel {} has status: {}", channel, so_channel, res);
            let _ = resp.send(res);
        }
        SetSoStatus {
            channel,
            so_channel,
            val,
        } => {
            let db = DB::open_default("data/autoso.db").expect("Could not open autoso.db");
            let key = format!("{channel} {so_channel}");
            let savable = if val { vec![1] } else { vec![0] };
            db.put(key, savable).expect("Cannot set online status");
        }
        ResetSoStatus { channel } => {
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
        }
        StartPrediction { channel, question } => {
            predictions.insert(
                channel,
                StreamPrediction {
                    name: question,
                    date: Utc::now(),
                    predictions: HashMap::new(),
                },
            );
        }
        PredictionProgress { channel, responses } => {
            let prediction = predictions.get_mut(&channel).expect("Should exist wtf");
            for (answer, responders) in responses {
                let answer = prediction.predictions.entry(answer).or_default();
                for (responder, points) in responders {
                    *answer.entry(responder).or_insert(0) = points;
                }
            }
        }
        PredictionEnd { channel } => {
            let prediction = predictions.get(&channel).expect("Should exist wtf");
            let filename = format!("data/{}_{}.json", prediction.name, prediction.date);
            let content = serde_json::to_string(&prediction.predictions);
            match content {
                Ok(str) => fs::write(filename, str).expect("Could not write"),
                Err(_) => {
                    error!("Could not parse prediction");
                }
            }
        }
        GetStreamInfo { channel, resp } => {
            let current = streams_data.entry(channel.clone()).or_insert(Event {
                id: None,
                broadcaster_user_id: channel.clone(),
                broadcaster_user_name: channel.clone(),
                online_type: None,
                started_at: None,
                title: None,
                language: None,
                category_id: None,
                category_name: None,
                broadcaster_user_login: channel,
                outcomes: None,
            });
            let _ = resp.send(current.clone());
        }
        SetStreamInfo { channel, event } => {
            streams_data.insert(channel, *event);
        }
        CountBits { channel, user, bits } => {
            let res = save_bits(channel.as_str(), user.as_str(), bits);
            if let Err(e) = res {
                error!("Could not save bits {:?}", e);
            }
        }
    }
}

fn save_bits(channel: &str, user: &str, bits: u64) -> Result<()> {
    let conn = Connection::open("data/data.db").with_context(|| "Could not open db")?;
    let now: DateTime<Local> = Local::now();
    conn.execute(
        "INSERT INTO bits VALUES (?, ?, ?, ?, ?)",
        (channel, now.to_rfc3339(), now.timestamp(), user, bits),
    )?;
    let _ = conn.close();
    Ok(())
}

fn initialize_db() -> Result<()> {
    let conn = Connection::open("data/data.db").with_context(|| "Could not open db")?;

    // Bits data
    conn.execute(
        "CREATE TABLE IF NOT EXISTS bits (
                channel TEXT NOT NULL,
                date TEXT NOT NULL,
                unix_time INTEGER NOT NULL,
                user TEXT NOT NULL,
                bits INTEGER NOT NULL
            )",
        (),
    )
    .with_context(|| "Could not create bits table".to_string())?;

    conn.execute(
        "CREATE INDEX IF NOT EXISTS channel_date_idx ON bits(channel, unix_time)",
        (),
    )
    .with_context(|| "Could not create index1".to_string())?;

    conn.execute("CREATE INDEX IF NOT EXISTS channel_user_idx ON bits(channel, user)", ())
        .with_context(|| "Could not create index2".to_string())?;

    conn.execute(
        "CREATE INDEX IF NOT EXISTS channel_user_date_idx ON bits (channel, unix_time, user)",
        (),
    )
    .with_context(|| "Could not create index3".to_string())?;
    Ok(())
}

pub fn create_state_manager(conf: Arc<BotConfig>, mut receiver: Receiver<Command>) -> JoinHandle<()> {
    tokio::spawn(async move {
        info!("Starting manager");
        let db = DB::open_default("data/online.db").expect("Could not open online.db");
        initialize_db().expect("Could not initialize db");

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

        let user_query = conf
            .channels
            .iter()
            .map(|c| c.channel_name.as_str())
            .map(|c| ("user_login", c))
            .collect::<Vec<_>>();
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

        let online_channels: ChannelsResponse = serde_json::from_str(&res).expect("Could not parse channel data");
        for c in online_channels.data {
            db.put(c.user_login, vec![1]).expect("Could not put online channels");
        }
        drop(db);
        info!("Starting to receive messages");
        let mut streams_data = HashMap::new();
        while let Some(cmd) = receiver.recv().await {
            process_command(cmd, &mut streams_data);
        }
    })
}
