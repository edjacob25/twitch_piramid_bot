use crate::bot_config::BotConfig;
use crate::bot_token_storage::CustomTokenStorage;
use crate::twitch_ws::Event;
use anyhow::{Context, Result};
use chrono::{DateTime, Local, Utc};
use log::{debug, error, info};
use reqwest::Client;
use rusqlite::{params, Connection};
use serde::Deserialize;
use std::collections::HashMap;
use std::fs;
use std::sync::Arc;
use tokio::sync::mpsc::Receiver;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use twitch_irc::login::TokenStorage;

type Responder<T> = oneshot::Sender<T>;

const DB_NAME: &str = "data/data.db";

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
    IncrementPyramid {
        channel: String,
        user: String,
        resp: Responder<i32>,
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
            let conn = Connection::open(DB_NAME).expect("Could not open db");
            let res = conn
                .query_row_and_then("SELECT online FROM channel WHERE name = ?1", [&key], |row| row.get(0))
                .unwrap_or_default();
            debug!("Channel {} online status: {}", key, res);
            let _ = resp.send(res);
        }
        SetChannelStatus { key, val } => {
            let conn = Connection::open(DB_NAME).expect("Could not open db");
            debug!("Setting channel {} online status: {}", key, val);
            if let Err(e) = conn.execute("UPDATE channel SET online = ?1 WHERE name = ?2", params![val, key]) {
                error!("Could not update channel status: {e}");
            }
        }
        GetSoStatus {
            channel,
            so_channel,
            resp,
        } => {
            let conn = Connection::open(DB_NAME).expect("Could not open db");
            let res = conn
                .query_row_and_then(
                    "SELECT done FROM autoso WHERE channel = ?1 AND recipient = ?2",
                    [&channel, &so_channel],
                    |row| row.get(0),
                )
                .unwrap_or_default();
            debug!("In channel {channel}, so_channel {so_channel} has status: {res}");
            let _ = resp.send(res);
        }
        SetSoStatus {
            channel,
            so_channel,
            val,
        } => {
            let conn = Connection::open(DB_NAME).expect("Could not open db");
            if let Err(e) = conn.execute(
                "INSERT INTO autoso VALUES (?1, ?2, ?3) ON CONFLICT(channel, recipient) DO UPDATE SET done='?3'",
                params![channel, so_channel, val],
            ) {
                error!("Could not set autoso status on channel {}: {}", channel, e);
            };
        }
        ResetSoStatus { channel } => {
            let conn = Connection::open(DB_NAME).expect("Could not open db");
            if let Err(e) = conn.execute("UPDATE autoso SET done = FALSE WHERE channel = ?1", [&channel]) {
                error!("Could not reset autoso status on channel {channel}: {e}");
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
        IncrementPyramid { channel, user, resp } => {
            let res = increment_pyramid_count(channel, user).unwrap_or_else(|e| {
                error!("Error when incrementing the pyramid: {e}");
                0
            });
            let _ = resp.send(res);
        }
    }
}

fn increment_pyramid_count(channel: String, user: String) -> Result<i32> {
    let conn = Connection::open(DB_NAME).expect("Could not open db");
    conn.execute(
        "INSERT INTO pyramids (channel, person) VALUES (?1, ?2) ON CONFLICT(channel, person) DO UPDATE SET count=count+1",
        params![channel, user],
    )
        .with_context(|| "Cannot increment pyramid count")?;
    let res = conn
        .query_row_and_then(
            "SELECT count FROM pyramids WHERE channel = ?1 AND person = ?2",
            [&channel, &user],
            |row| row.get(0),
        )
        .with_context(|| "Could not retrieve pyramid count")?;
    Ok(res)
}

fn save_bits(channel: &str, user: &str, bits: u64) -> Result<()> {
    let conn = Connection::open(DB_NAME).with_context(|| "Could not open db")?;
    let now: DateTime<Local> = Local::now();
    conn.execute(
        "INSERT INTO bits VALUES (?, ?, ?, ?, ?)",
        (channel, now.to_rfc3339(), now.timestamp(), user, bits),
    )?;
    let _ = conn.close();
    Ok(())
}

fn initialize_db() -> Result<()> {
    let conn = Connection::open(DB_NAME).with_context(|| "Could not open db")?;

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

    // Pyramids data
    conn.execute(
        "CREATE TABLE IF NOT EXISTS pyramids (
            channel TEXT NOT NULL,
            person TEXT NOT NULL,
            count INTEGER DEFAULT 1
        )",
        (),
    )
    .with_context(|| "Could not create pyramids table".to_string())?;

    conn.execute(
        "CREATE UNIQUE INDEX IF NOT EXISTS pyramids_idx ON pyramids (channel, person)",
        (),
    )
    .with_context(|| "Could not create pyramids index".to_string())?;

    // Online data
    conn.execute(
        "CREATE TABLE IF NOT EXISTS channel (
            name TEXT PRIMARY KEY,
            online BOOLEAN NOT NULL
        )",
        (),
    )
    .with_context(|| "Could not create channel table".to_string())?;

    // AutoSO
    conn.execute(
        "CREATE TABLE IF NOT EXISTS autoso (
            channel TEXT NOT NULL,
            recipient TEXT NOT NULL,
            done BOOLEAN NOT NULL
        )",
        (),
    )
    .with_context(|| "Could not create autoso table".to_string())?;

    conn.execute(
        "CREATE UNIQUE INDEX IF NOT EXISTS autoso_idx ON autoso (channel, recipient)",
        (),
    )
    .with_context(|| "Could not create autoso index".to_string())?;
    Ok(())
}

pub fn create_state_manager(conf: Arc<BotConfig>, mut receiver: Receiver<Command>) -> JoinHandle<()> {
    tokio::spawn(async move {
        info!("Starting manager");
        initialize_db().expect("Could not initialize db");

        let conn = Connection::open(DB_NAME).expect("Could not open db");
        conf.channels
            .iter()
            .map(|c| c.channel_name.as_str())
            .for_each(|channel| {
                if let Err(e) = conn.execute(
                    "INSERT INTO channel VALUES (?1, FALSE) ON CONFLICT(name) DO UPDATE SET online=FALSE",
                    [channel],
                ) {
                    error!("Could not insert {channel}: {e}");
                };
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
            if let Err(e) = conn.execute("UPDATE channel SET online = TRUE WHERE name = ?1", [&c.user_login]) {
                error!("Could not update channel {} status: {}", c.user_login, e);
            }
        }
        conn.close().expect("Could not close DB");
        info!("Starting to receive messages");
        let mut streams_data = HashMap::new();
        while let Some(cmd) = receiver.recv().await {
            process_command(cmd, &mut streams_data);
        }
    })
}
