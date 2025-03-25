use crate::bot_config::BotConfig;
use crate::bot_token_storage::CustomTokenStorage;
use crate::events::twitch_ws::Event;
use anyhow::{Context, Result};
use chrono::{DateTime, Local, Utc};
use command::Command;
use data::*;
use log::{debug, error, info};
use reqwest::Client;
use rusqlite::{Connection, params};
use serde::Deserialize;
use std::cmp::PartialEq;
use std::collections::HashMap;
use std::fs;
use std::sync::Arc;
use tokio::sync::broadcast::Sender;
use tokio::sync::mpsc::Receiver;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use twitch_irc::login::TokenStorage;

pub mod command;
pub mod data;
mod queue;

type Responder<T> = oneshot::Sender<T>;

const DB_NAME: &str = "data/data.db";

#[derive(Debug, PartialEq)]
pub enum Source {
    Chat,
    Web,
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

struct StateManager {
    conf: Arc<BotConfig>,
    receiver: Receiver<Command>,
    sender: Sender<(Queue, String)>,
    streams_data: HashMap<String, Event>,
}

impl StateManager {
    fn process_command(&mut self, cmd: Command) {
        let mut predictions: HashMap<String, StreamPrediction> = HashMap::new();
        use command::Command::*;
        match cmd {
            // ChannelStatus
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
            // AutoSo
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
            // Predictions
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
            // Stream info
            GetStreamInfo { channel, resp } => {
                let current = self.streams_data.entry(channel.clone()).or_insert(Event {
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
                self.streams_data.insert(channel, *event);
            }
            // Buts
            CountBits { channel, user, bits } => {
                let res = Self::save_bits(&channel, &user, bits);
                if let Err(e) = res {
                    error!("Could not save bits {:?}", e);
                }
            }
            // Pyramids
            IncrementPyramid { channel, user, resp } => {
                let res = Self::increment_pyramid_count(&channel, &user).unwrap_or_else(|e| {
                    error!("Error when incrementing the pyramid: {e}");
                    0
                });
                let _ = resp.send(res);
            }
            // Queues
            CreateQueue {
                channel,
                teams,
                per_team,
            } => {
                if let Err(e) = self.create_queue(&channel, teams, per_team) {
                    error!("Could not create queue for channel {channel}: {e}");
                };
            }
            ResetQueue { channel } => {
                if let Err(e) = self.create_queue(&channel, 0, 0) {
                    error!("Could not reset queue for channel {channel}: {e}");
                };
            }
            AddToQueue {
                channel,
                user,
                second_user,
                team,
                source,
                resp,
            } => match self.add_to_queue(&channel, user.clone(), second_user, team, source) {
                Ok(res) => resp.send(res).unwrap_or_default(),
                Err(e) => {
                    error!("Could not add user(s) {user} {channel}: {e}");
                    let _ = resp.send(AddResult::GeneralError);
                }
            },
            ConfirmUser { channel, user, resp } => match self.confirm_user(&channel, &user) {
                Ok(r) => {
                    let _ = resp.send(r);
                }
                Err(e) => {
                    error!("Could not confirm user {user} {channel}: {e}");
                    let _ = resp.send(ConfirmResult::GeneralError);
                }
            },
            RemoveFromQueue {
                channel,
                user,
                source,
                resp,
            } => match self.delete_from_queue(&channel, &user, source) {
                Ok(res) => resp.send(res).unwrap_or_default(),
                Err(e) => {
                    error!("Could not delete user {user} {channel}: {e}");
                    let _ = resp.send(DeletionResult::GeneralError);
                }
            },
            MoveToOtherTeam {
                channel,
                user,
                team,
                source,
                resp,
            } => match self.move_to_other_team(&channel, &user, team, source) {
                Ok(res) => resp.send(res).unwrap_or_default(),
                Err(e) => {
                    error!("Could not move user {user} {channel}: {e}");
                    let _ = resp.send(MoveResult::GeneralError);
                }
            },
            ShowQueue { channel, resp } => match Self::get_queue(&channel) {
                Ok(res) => {
                    debug!("Queue from db {:?}", res);
                    let _ = resp.send(res);
                }
                Err(err) => {
                    error!("Could not get queue from db {:?}", err);
                    let _ = resp.send(Queue::default());
                }
            },
            SwitchQueueStatus { channel, resp } => {
                resp.send(self.switch_queue(&channel)).unwrap_or_default();
            }
        }
    }

    fn increment_pyramid_count(channel: &str, user: &str) -> Result<i32> {
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

    async fn run(&mut self) {
        let conn = Connection::open(DB_NAME).expect("Could not open db");
        self.conf
            .channels
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
            location: self.conf.credentials_file.clone(),
        };

        let user_query = self
            .conf
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
            .header("Client-Id", self.conf.client_id.as_str())
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
        while let Some(cmd) = self.receiver.recv().await {
            self.process_command(cmd);
        }
    }

    fn new(conf: Arc<BotConfig>, receiver: Receiver<Command>, sender: Sender<(Queue, String)>) -> StateManager {
        info!("Starting manager");
        initialize_db().expect("Could not initialize db");

        let streams_data = HashMap::new();
        StateManager {
            conf,
            receiver,
            sender,
            streams_data,
        }
    }
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

    // Teams
    conn.execute(
        "CREATE TABLE IF NOT EXISTS queue (
            channel TEXT PRIMARY KEY,
            no_teams INTEGER NOT NULL,
            team_size INTEGER NOT NULL,
            teams TEXT NOT NULL,
            active BOOLEAN NOT NULL
        )",
        (),
    )
    .with_context(|| "Could not create queue table".to_string())?;

    Ok(())
}

pub fn create_state_manager(
    conf: Arc<BotConfig>,
    receiver: Receiver<Command>,
    sender: Sender<(Queue, String)>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        StateManager::new(conf, receiver, sender).run().await;
    })
}
