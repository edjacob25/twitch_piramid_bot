use crate::bot_config::BotConfig;
use crate::bot_token_storage::CustomTokenStorage;
use crate::teams::Status::{Confirmed, Unconfirmed};
use crate::teams::{Member, Queue, Team};
use crate::twitch_ws::Event;
use anyhow::{Context, Result, bail};
use chrono::{DateTime, Local, Utc};
use log::{debug, error, info};
use reqwest::Client;
use rusqlite::{Connection, params};
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
    CreateQueue {
        channel: String,
        teams: u8,
        per_team: u8,
    },
    ResetQueue {
        channel: String,
    },
    AddToQueue {
        channel: String,
        user: String,
        second_user: Option<String>,
        team: Option<u8>,
        resp: Responder<bool>,
    },
    ConfirmUser {
        channel: String,
        user: String,
    },
    RemoveFromQueue {
        channel: String,
        user: String,
    },
    MoveToOtherTeam {
        channel: String,
        user: String,
        team: u8,
    },
    ShowQueue {
        channel: String,
        resp: Responder<Queue>,
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
        // Buts
        CountBits { channel, user, bits } => {
            let res = save_bits(channel.as_str(), user.as_str(), bits);
            if let Err(e) = res {
                error!("Could not save bits {:?}", e);
            }
        }
        // Pyramids
        IncrementPyramid { channel, user, resp } => {
            let res = increment_pyramid_count(channel, user).unwrap_or_else(|e| {
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
            if let Err(e) = create_queue(&channel, teams, per_team) {
                error!("Could not create queue for channel {channel}: {e}");
            };
        }
        ResetQueue { channel } => {
            if let Err(e) = create_queue(&channel, 0, 0) {
                error!("Could not reset queue for channel {channel}: {e}");
            };
        }
        AddToQueue {
            channel,
            user,
            second_user,
            team,
            resp,
        } => match add_to_queue(&channel, user.clone(), second_user, team) {
            Ok(_) => resp.send(true).unwrap_or_default(),
            Err(e) => {
                error!("Could not add user(s) {user} {}: {}", channel, e);
                let _ = resp.send(false);
            }
        },
        ConfirmUser { channel, user } => {
            if let Err(e) = confirm_user(&channel, &user) {
                error!("Could not confirm user(s) {user} {}: {}", channel, e);
            }
        }
        RemoveFromQueue { channel, user } => {
            if let Err(e) = delete_from_queue(&channel, &user) {
                error!("Could not delete user(s) {user} {}: {}", channel, e);
            }
        }
        MoveToOtherTeam { .. } => {}
        ShowQueue { channel, resp } => match get_queue(&channel) {
            Ok(res) => {
                debug!("Queue from db {:?}", res);
                let _ = resp.send(res);
            }
            Err(err) => {
                error!("Could not get queue from db {:?}", err);
                let _ = resp.send(Queue::default());
            }
        },
    }
}

fn create_queue(channel: &str, teams: u8, per_team: u8) -> Result<()> {
    let conn = Connection::open(DB_NAME)?;
    let teams_vec = (0..teams).into_iter().map(|_| Team::default()).collect::<Vec<_>>();
    let json = serde_json::to_string(&teams_vec)?;
    if let Err(e) = conn.execute(
        "INSERT INTO queue VALUES (?1, ?2, ?3, $4) ON CONFLICT(channel) DO UPDATE SET no_teams=?2, team_size=?3, teams=$4",
        params![channel, teams, per_team, json],
    ) {
        bail!("Db error when creating queue: {}", e);
    };
    Ok(())
}

fn get_queue(channel: &str) -> Result<Queue> {
    let conn = Connection::open(DB_NAME)?;
    conn.query_row_and_then(
        "SELECT no_teams, team_size, teams FROM queue WHERE channel = ?1",
        [channel],
        |row| {
            let json: String = row.get(2)?;
            let teams: Vec<Team> = serde_json::from_str(&json)?;
            Ok(Queue {
                size: row.get(0)?,
                team_size: row.get(1)?,
                teams,
            })
        },
    )
}

fn update_queue(channel: &str, queue: Queue, operation: &str) -> Result<()> {
    let conn = Connection::open(DB_NAME)?;
    let json = serde_json::to_string(&queue.teams)?;
    if let Err(e) = conn.execute(
        "UPDATE queue SET teams = json(?2) WHERE channel = ?1",
        params![channel, json],
    ) {
        bail!("Db error when {operation} user to channel {channel}: {e}");
    };
    Ok(())
}

fn add_to_queue(channel: &str, user: String, second_user: Option<String>, preferred_team: Option<u8>) -> Result<()> {
    let mut queue = get_queue(channel)?;
    let mut users = 2u8;
    let second_user = second_user.unwrap_or_else(|| {
        users = 1;
        "".to_string()
    });

    let mut free_spaces = vec![];
    let mut found_already = false;
    for team in queue.teams.iter() {
        free_spaces.push(queue.team_size - team.members.len() as u8);
        let names = team.members.iter().map(|x| x.name.as_str()).collect::<Vec<_>>();
        if names.contains(&user.as_str()) || (users == 2 && names.contains(&second_user.as_str())) {
            found_already = true;
        }
    }
    if found_already {
        bail!("Already found in queue");
    }

    let mut chosen_idx = None;
    if let Some(preferred_team) = preferred_team {
        let real_idx = (preferred_team - 1) as usize;
        if free_spaces[real_idx] >= users {
            chosen_idx = Some(real_idx);
        }
    }

    if chosen_idx.is_none() {
        for (idx, team_free_space) in free_spaces.iter().enumerate() {
            if *team_free_space >= users {
                chosen_idx = Some(idx);
                break;
            }
        }
    }
    if let Some(chosen_idx) = chosen_idx {
        queue.teams[chosen_idx].members.push(Member {
            name: user,
            status: Confirmed,
        });
        if users == 2 {
            queue.teams[chosen_idx].members.push(Member {
                name: second_user.to_string(),
                status: Unconfirmed,
            });
        }
        update_queue(channel, queue, "Adding")?;
    } else {
        bail!("Could not join any team");
    }
    Ok(())
}

fn confirm_user(channel: &str, user: &str) -> Result<()> {
    let mut queue = get_queue(channel)?;
    let mut found = false;
    for team in queue.teams.iter_mut() {
        for member in team.members.iter_mut() {
            if member.name == user && member.status == Unconfirmed {
                member.status = Confirmed;
                found = true;
                break;
            }
        }
    }
    if !found {
        bail!("Could not confirm user {user} for channel {channel}");
    } else {
        update_queue(channel, queue, "confirming")?;
    }
    Ok(())
}

fn delete_from_queue(channel: &str, user: &str) -> Result<()> {
    let mut queue = get_queue(channel)?;
    let mut found = false;
    for team in queue.teams.iter_mut() {
        for (idx, member) in team.members.iter().enumerate() {
            if member.name == user {
                team.members.remove(idx);
                found = true;
                break;
            }
        }
    }

    if !found {
        bail!("Could not delete user {user} for channel {channel}");
    } else {
        update_queue(channel, queue, "deleting")?;
    }
    Ok(())
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

    // Teams
    conn.execute(
        "CREATE TABLE IF NOT EXISTS queue (
            channel TEXT PRIMARY KEY,
            no_teams INTEGER NOT NULL,
            team_size INTEGER NOT NULL,
            teams TEXT NOT NULL
        )",
        (),
    )
    .with_context(|| "Could not create queue table".to_string())?;

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
