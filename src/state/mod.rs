use crate::bot_config::BotConfig;
use crate::bot_token_storage::CustomTokenStorage;
use crate::events::twitch_ws::Event;
use crate::teams::Status::{Confirmed, Unconfirmed};
use crate::teams::{AddResult, ConfirmResult, DeletionResult, Member, MoveResult, Queue, Team};
use anyhow::{Context, Result, bail};
use chrono::{DateTime, Local, Utc};
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

type Responder<T> = oneshot::Sender<T>;

const DB_NAME: &str = "data/data.db";

#[derive(Debug, PartialEq)]
pub enum Source {
    Chat,
    Web,
}

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
        source: Source,
        resp: Responder<AddResult>,
    },
    ConfirmUser {
        channel: String,
        user: String,
        resp: Responder<ConfirmResult>,
    },
    RemoveFromQueue {
        channel: String,
        user: String,
        source: Source,
        resp: Responder<DeletionResult>,
    },
    MoveToOtherTeam {
        channel: String,
        user: String,
        team: u8,
        source: Source,
        resp: Responder<MoveResult>,
    },
    ShowQueue {
        channel: String,
        resp: Responder<Queue>,
    },
    SwitchQueueStatus {
        channel: String,
        resp: Responder<Result<bool>>,
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

struct StateManager {
    conf: Arc<BotConfig>,
    receiver: Receiver<Command>,
    sender: Sender<(Queue, String)>,
    streams_data: HashMap<String, Event>,
}

impl StateManager {
    fn process_command(&mut self, cmd: Command) {
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
    fn create_queue(&self, channel: &str, teams: u8, per_team: u8) -> Result<()> {
        info!("Creating queue for channel {channel} with {teams} teams and {per_team} spaces per team");
        let conn = Connection::open(DB_NAME)?;
        let teams_vec = (0..teams).map(|_| Team::default()).collect::<Vec<_>>();
        let json = serde_json::to_string(&teams_vec)?;
        if let Err(e) = conn.execute(
            "INSERT INTO queue VALUES (?1, ?2, ?3, $4, $5) ON CONFLICT(channel) DO UPDATE SET no_teams=?2, team_size=?3, teams=$4, active=$5",
            params![channel, teams, per_team, json, true],
        ) {
            bail!("Db error when creating queue: {}", e);
        };
        let q = Queue {
            size: teams,
            team_size: per_team,
            teams: teams_vec,
            active: true,
        };
        self.sender.send((q, channel.to_string())).ok();
        Ok(())
    }

    fn get_queue(channel: &str) -> Result<Queue> {
        info!("Getting queue for channel {channel}");
        let conn = Connection::open(DB_NAME)?;
        conn.query_row_and_then(
            "SELECT no_teams, team_size, teams, active FROM queue WHERE channel = ?1",
            [channel],
            |row| {
                let json: String = row.get(2)?;
                let teams: Vec<Team> = serde_json::from_str(&json)?;
                Ok(Queue {
                    size: row.get(0)?,
                    team_size: row.get(1)?,
                    active: row.get(3)?,
                    teams,
                })
            },
        )
    }

    fn update_queue(&self, channel: &str, queue: Queue, operation: &str) -> Result<()> {
        let conn = Connection::open(DB_NAME)?;
        let json = serde_json::to_string(&queue.teams)?;
        if let Err(e) = conn.execute(
            "UPDATE queue SET teams = json(?2), active=?3 WHERE channel = ?1",
            params![channel, json, queue.active],
        ) {
            bail!("Db error when {operation} user to channel {channel}: {e}");
        };
        self.sender.send((queue, channel.to_string())).ok();
        Ok(())
    }

    fn add_to_queue(
        &self,
        channel: &str,
        user: String,
        second_user: Option<String>,
        pref_team: Option<u8>,
        source: Source,
    ) -> Result<AddResult> {
        info!("Adding {user} to channel {channel}");
        let mut queue = Self::get_queue(channel)?;
        if source == Source::Chat && !queue.active {
            return Ok(AddResult::QueueFrozen);
        }

        let mut users = 2u8;
        let second_user = second_user.unwrap_or_else(|| {
            users = 1;
            String::new()
        });

        let mut free_spaces = vec![];
        let mut already_found = false;
        for team in &queue.teams {
            free_spaces.push(queue.team_size - team.members.len() as u8);
            let names = team.members.iter().map(|x| x.name.as_str()).collect::<Vec<_>>();
            if names.contains(&user.as_str()) || (users == 2 && names.contains(&second_user.as_str())) {
                already_found = true;
            }
        }
        if already_found {
            return Ok(AddResult::AlreadyInQueue);
        }

        let mut chosen_idx = None;
        if let Some(preferred_team) = pref_team {
            let real_idx = preferred_team as usize;
            if real_idx < free_spaces.len() && free_spaces[real_idx] >= users {
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
            self.update_queue(channel, queue, "Adding")?;
            Ok(AddResult::Success(chosen_idx))
        } else {
            Ok(AddResult::NoSpace)
        }
    }

    fn confirm_user(&self, channel: &str, user: &str) -> Result<ConfirmResult> {
        info!("Confirming {user} in channel {channel}");
        let mut queue = Self::get_queue(channel)?;
        let mut found = false;
        let mut idx = 0;
        for team in &mut queue.teams {
            for member in &mut team.members {
                if member.name == user && member.status == Unconfirmed {
                    member.status = Confirmed;
                    found = true;
                    break;
                }
            }
            idx += 1;
        }
        if found {
            self.update_queue(channel, queue, "confirming")?;
            Ok(ConfirmResult::Success(idx))
        } else {
            Ok(ConfirmResult::NotFound)
        }
    }

    fn move_to_other_team(&self, channel: &str, user: &str, desired_team: u8, source: Source) -> Result<MoveResult> {
        info!("Moving {user} to team {desired_team} in channel {channel}");
        let mut queue = Self::get_queue(channel)?;
        if source == Source::Chat && !queue.active {
            return Ok(MoveResult::QueueFrozen);
        }
        if desired_team as usize >= queue.teams.len() {
            return Ok(MoveResult::InvalidTeam);
        }
        let available_space = queue.teams[desired_team as usize].members.len() < queue.team_size as usize;
        if !available_space {
            return Ok(MoveResult::NoSpace);
        }
        let mut final_idx = None;
        for (t_idx, team) in queue.teams.iter().enumerate() {
            for (idx, member) in team.members.iter().enumerate() {
                if member.name == user {
                    final_idx = Some((t_idx, idx));
                    break;
                }
            }
        }

        if let Some(final_idx) = final_idx {
            if final_idx.0 == desired_team as usize {
                return Ok(MoveResult::AlreadyInTeam);
            }
            let p = queue.teams[final_idx.0].members.remove(final_idx.1);
            queue.teams[desired_team as usize].members.push(p);

            self.update_queue(channel, queue, "moving")?;
            Ok(MoveResult::Success)
        } else {
            Ok(MoveResult::NotFound)
        }
    }

    fn delete_from_queue(&self, channel: &str, user: &str, source: Source) -> Result<DeletionResult> {
        info!("Deleting {user} in channel {channel}");
        let mut queue = Self::get_queue(channel)?;
        if source == Source::Chat && !queue.active {
            return Ok(DeletionResult::QueueFrozen);
        }
        let mut found = false;
        for team in &mut queue.teams {
            for (idx, member) in team.members.iter().enumerate() {
                if member.name == user {
                    team.members.remove(idx);
                    found = true;
                    break;
                }
            }
        }

        if found {
            self.update_queue(channel, queue, "deleting")?;
            Ok(DeletionResult::Success)
        } else {
            Ok(DeletionResult::NotFound)
        }
    }

    fn switch_queue(&self, channel: &str) -> Result<bool> {
        info!("Freezing/unfreezing queue for channel {channel}");
        let mut queue = Self::get_queue(channel)?;
        let result = !queue.active;
        queue.active = result;
        self.update_queue(channel, queue, "freezing/unfreezing")?;
        Ok(result)
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
