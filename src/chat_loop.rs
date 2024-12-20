use crate::bot_config::{BotConfig, ChannelConfig};
use crate::bot_token_storage::CustomTokenStorage;
use crate::chat_action::ChatAction;
use crate::pyramid_action::PyramidAction;
use crate::state_manager::Command;
use governor::clock::DefaultClock;
use governor::state::keyed::DefaultKeyedStateStore;
use governor::state::RateLimiter;
use governor::Quota;
use log::{debug, info, warn};
use rocksdb::DB;
use std::collections::HashMap;
use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc::{Sender, UnboundedReceiver};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use twitch_irc::client::TwitchIRCClient;
use twitch_irc::login::RefreshingLoginCredentials;
use twitch_irc::message::{PrivmsgMessage as ChatMessage, ReplyToMessage, ServerMessage};
use twitch_irc::transport::websocket::SecureWSTransport;

type TwitchClient =
    TwitchIRCClient<SecureWSTransport, RefreshingLoginCredentials<CustomTokenStorage>>;
type Limiter = RateLimiter<String, DefaultKeyedStateStore<String>, DefaultClock>;

struct PyramidData {
    building_flags: HashMap<String, bool>,
    emote_counts: HashMap<String, usize>,
    emotes: HashMap<String, String>,
}

pub fn message_loop(
    conf: Arc<BotConfig>,
    incoming_messages: UnboundedReceiver<ServerMessage>,
    cl: TwitchClient,
    sender: Sender<Command>,
) -> JoinHandle<()> {
    let join_handle = tokio::spawn(async move {
        info!("Starting chat");
        ChatLoop::new(conf, incoming_messages, cl, sender)
            .run()
            .await;
    });
    join_handle
}

struct ChatLoop {
    incoming_messages: UnboundedReceiver<ServerMessage>,
    cl: TwitchClient,
    sender: Sender<Command>,
    pyramids: PyramidData,
    channel_configs: HashMap<String, ChannelConfig>,
    limiter: Limiter,
}

impl ChatLoop {
    fn new(
        conf: Arc<BotConfig>,
        incoming_messages: UnboundedReceiver<ServerMessage>,
        cl: TwitchClient,
        sender: Sender<Command>,
    ) -> ChatLoop {
        let mut pyramids = PyramidData {
            building_flags: HashMap::new(),
            emote_counts: HashMap::new(),
            emotes: HashMap::new(),
        };

        for channel_to_connect in &conf.channels {
            let channel_name = &channel_to_connect.channel_name;
            pyramids.building_flags.insert(channel_name.clone(), false);
            pyramids.emote_counts.insert(channel_name.clone(), 0usize);
            pyramids.emotes.insert(channel_name.clone(), "".to_string());
        }

        let channel_configs = conf.channels.iter().fold(HashMap::new(), |mut acc, c| {
            acc.insert(c.channel_name.clone(), c.clone());
            acc
        });

        let limiter = RateLimiter::keyed(
            Quota::with_period(Duration::from_secs_f32(1.5))
                .unwrap()
                .allow_burst(NonZeroU32::new(20).unwrap()),
        );

        ChatLoop {
            incoming_messages,
            cl,
            sender,
            pyramids,
            channel_configs,
            limiter,
        }
    }

    async fn say_rate_limited(&self, channel: &str, msg: String) {
        let channel = channel.to_string();
        match self.limiter.check_key(&channel) {
            Ok(_) => self.cl.say(channel, msg).await.unwrap(),
            Err(_) => {
                warn!("Rate limited")
            }
        }
    }

    async fn do_pyramid_action(&self, channel: &str, emote: &str) {
        let action: PyramidAction = rand::random();
        match action {
            PyramidAction::Steal => {
                self.say_rate_limited(channel, emote.to_string()).await;
            }
            PyramidAction::Destroy => {
                self.say_rate_limited(channel, "No".to_string()).await;
            }
            _ => warn!("Do nothing"),
        }
    }

    async fn respond_something(&self, msg: &ChatMessage) {
        let config = self
            .channel_configs
            .get(msg.channel_login.as_str())
            .unwrap();

        if let Some(c) = config.automatic_responses.as_ref() {
            for pair in c {
                let regex = &pair.regex;
                let response = pair.response.clone();
                if regex.is_match(msg.message_text.as_str()) {
                    self.say_rate_limited(msg.channel_login(), response).await;
                }
            }
        }
    }

    async fn do_pyramid_counting(&self, msg: &ChatMessage) {
        if msg.sender.name != "StreamElements" || !msg.message_text.contains("pirámide") {
            return;
        }
        let (tx, rx) = oneshot::channel();
        let cmd = Command::GetChannelStatus {
            key: msg.channel_login.clone(),
            resp: tx,
        };
        let _ = self.sender.send(cmd).await;
        let is_online = rx.await.unwrap_or_else(|_| false);

        if !is_online {
            return;
        }

        let as_vec = msg.message_text.split(" ").collect::<Vec<_>>();
        let name = as_vec[as_vec.len() - 2];
        let db = DB::open_default("data/pyramids.db").unwrap();
        let combined = format!("{} {}", msg.channel_login, name);
        let mut num: u32 = match db.get(combined.as_bytes()) {
            Ok(Some(value)) => String::from_utf8(value).unwrap().parse().unwrap(),
            Ok(None) => 0,
            Err(_) => 0,
        };
        num += 1;
        db.put(combined, format!("{}", num)).expect("Error with db");
        let message = format!("{} lleva {} piramides", name, num);
        self.say_rate_limited(msg.channel_login.as_str(), message)
            .await;
    }

    async fn do_pyramid_interference(&mut self, msg: &ChatMessage) {
        let channel = msg.channel_login.as_str();
        let config = self.channel_configs.get(channel).unwrap();

        let pyramid_building = self.pyramids.building_flags.get_mut(channel).unwrap();
        let emote_count = self.pyramids.emote_counts.get_mut(channel).unwrap();
        let emote = self.pyramids.emotes.get_mut(channel).unwrap();

        if !msg.message_text.contains(" ") {
            *pyramid_building = true;
            *emote_count = 1;
            *emote = msg.message_text.clone();
            info!("Single word {}", *emote);
            return;
        }
        if !*pyramid_building {
            return;
        }
        let emote = emote.clone();
        let num_of_matches = msg
            .message_text
            .match_indices(&emote)
            .collect::<Vec<_>>()
            .len();
        let num_of_words = msg.message_text.split(" ").collect::<Vec<_>>().len();
        if num_of_words != num_of_matches {
            *pyramid_building = false;
            return;
        }
        match num_of_matches {
            i if i == *emote_count + 1 => {
                info!("Pyramid growing");
                *emote_count += 1;

                if let Some(h) = config.harder_pyramids.as_ref() {
                    if h.contains(&msg.sender.name)
                        && *emote_count == 3
                        && rand::random::<f32>() < 0.5
                    {
                        warn!("Taking it hard");
                        // self.say_rate_limited(&msg.channel_login, "No".to_string()).await;
                        *pyramid_building = false;
                    }
                }
            }
            i if i == *emote_count - 1 => {
                info!("Pyramid getting smaller");
                *emote_count -= 1;
                if *emote_count != 2 {
                    return;
                }
                *pyramid_building = false;

                if let Some(h) = config.easier_pyramids.as_ref() {
                    if h.contains(&msg.sender.name) && rand::random::<f32>() < 0.5 {
                        warn!("Taking it easy");
                        return;
                    }
                }

                warn!("Time to strike");
                self.do_pyramid_action(channel, &emote).await;
            }
            _ => *pyramid_building = false,
        }
    }

    async fn do_auto_so(&self, msg: &ChatMessage) {
        let config = self.channel_configs.get(msg.channel_login()).unwrap();

        match &config.auto_so_channels {
            Some(auto_so_channels) if auto_so_channels.contains(&msg.sender.name) => {
                info!("{} {}", msg.channel_login, msg.sender.name);
                let (tx, rx) = oneshot::channel();
                let cmd = Command::GetSoStatus {
                    channel: msg.channel_login.clone(),
                    so_channel: msg.sender.name.clone(),
                    resp: tx,
                };
                self.sender
                    .send(cmd)
                    .await
                    .expect("Could not send request for so status");
                let already_sod = rx.await.unwrap_or_else(|_| true);
                if !already_sod {
                    self.say_rate_limited(&msg.channel_login, format!("!so {}", msg.sender.name))
                        .await;
                    let (tx, rx) = oneshot::channel();
                    let cmd = Command::SetSoStatus {
                        channel: msg.channel_login.clone(),
                        so_channel: msg.sender.name.clone(),
                        val: true,
                        resp: tx,
                    };
                    self.sender
                        .send(cmd)
                        .await
                        .expect("Could not send request for so status");
                    assert_eq!(rx.await.unwrap(), ())
                }
            }
            _ => {}
        }
    }

    async fn process_twitch_message(&mut self, message: ServerMessage) {
        match message {
            ServerMessage::Privmsg(msg) => {
                info!(
                    "(#{}) {}: {}",
                    msg.channel_login, msg.sender.name, msg.message_text
                );

                let actions = self
                    .channel_configs
                    .get(&msg.channel_login)
                    .unwrap()
                    .permitted_actions
                    .clone();

                for action in actions.iter() {
                    match action {
                        ChatAction::RespondSomething => {
                            self.respond_something(&msg).await;
                        }
                        ChatAction::PyramidCounting => {
                            self.do_pyramid_counting(&msg).await;
                        }
                        ChatAction::PyramidInterference => {
                            self.do_pyramid_interference(&msg).await;
                        }
                        ChatAction::AutoSO => {
                            self.do_auto_so(&msg).await;
                        }
                        _ => {}
                    }
                }
            }
            ServerMessage::UserNotice(msg) => {
                let channel_conf = self.channel_configs.get(&msg.channel_login).unwrap();

                if !channel_conf.permitted_actions.contains(&ChatAction::GiveSO) {
                    return;
                }
                if msg.event_id != "raid" {
                    return;
                }
                self.say_rate_limited(&msg.channel_login, format!("!so @{}", msg.sender.login))
                    .await;
                debug!("{:?}", msg)
            }
            _ => {
                debug!("{:?}", message)
            }
        }
    }

    async fn run(&mut self) {
        info!("Starting chat");
        while let Some(message) = self.incoming_messages.recv().await {
            self.process_twitch_message(message).await;
        }
    }
}
