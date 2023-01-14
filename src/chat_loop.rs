use crate::bot_config::{BotConfig, ChannelConfig};
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
use std::time::Duration;
use tokio::sync::mpsc::{Sender, UnboundedReceiver};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use twitch_irc::client::TwitchIRCClient;
use twitch_irc::login::StaticLoginCredentials;
use twitch_irc::message::ServerMessage;
use twitch_irc::transport::websocket::SecureWSTransport;

type TwitchClient = TwitchIRCClient<SecureWSTransport, StaticLoginCredentials>;
type Limiter = RateLimiter<String, DefaultKeyedStateStore<String>, DefaultClock>;

struct ClientCombo {
    client: TwitchClient,
    limiter: Limiter,
}

struct PyramidData {
    building_flags: HashMap<String, bool>,
    emote_counts: HashMap<String, usize>,
    emotes: HashMap<String, String>,
}

async fn say_rate_limited(combo: &ClientCombo, channel: &str, msg: String) {
    let channel = channel.to_string();
    match combo.limiter.check_key(&channel) {
        Ok(_) => combo.client.say(channel, msg).await.unwrap(),
        Err(_) => {
            warn!("Rate limited")
        }
    }
}

async fn do_ayy(combo: &ClientCombo, msg: &twitch_irc::message::PrivmsgMessage) {
    if msg.message_text.to_lowercase().contains("ayy") {
        say_rate_limited(combo, msg.channel_login.as_str(), "lmao".to_string()).await;
    }
}

async fn do_pyramid_counting(
    combo: &ClientCombo,
    msg: &twitch_irc::message::PrivmsgMessage,
    sender: &Sender<Command>,
) {
    if msg.sender.name == "StreamElements" && msg.message_text.contains("pirámide") {
        let (tx, rx) = oneshot::channel();
        let cmd = Command::GetChannelStatus {
            key: msg.channel_login.clone(),
            resp: tx,
        };
        let _ = sender.send(cmd).await;
        let is_online = match rx.await {
            Ok(res) => res,
            Err(_) => false,
        };

        if !is_online {
            return;
        }

        let as_vec = msg.message_text.split(" ").collect::<Vec<_>>();
        let name = as_vec[as_vec.len() - 2];
        let db = DB::open_default("pyramids.db").unwrap();
        let combined = format!("{} {}", msg.channel_login, name);
        let mut num: u32 = match db.get(combined.as_bytes()) {
            Ok(Some(value)) => String::from_utf8(value).unwrap().parse().unwrap(),
            Ok(None) => 0,
            Err(_) => 0,
        };
        num += 1;
        db.put(combined, format!("{}", num)).expect("Error with db");
        let message = format!("{} lleva {} piramides", name, num);
        say_rate_limited(combo, msg.channel_login.as_str(), message).await;
    }
}

async fn do_pyramid_interference(
    combo: &ClientCombo,
    msg: &twitch_irc::message::PrivmsgMessage,
    pyramids: &mut PyramidData,
) {
    let channel = msg.channel_login.as_str();
    let pyramid_building = pyramids.building_flags.get_mut(channel).unwrap();
    let emote_count = pyramids.emote_counts.get_mut(channel).unwrap();
    let emote = pyramids.emotes.get_mut(channel).unwrap();

    if !msg.message_text.contains(" ") {
        *pyramid_building = true;
        *emote_count = 1;
        *emote = msg.message_text.clone();
        info!("Single word {}", *emote);
    } else if *pyramid_building {
        let num_of_matches = msg
            .message_text
            .match_indices(emote.as_str())
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
            }
            i if i == *emote_count - 1 => {
                info!("Pyramid getting smaller");
                *emote_count -= 1;
                if *emote_count == 2 {
                    warn!("Time to strike");
                    do_pyramid_action(combo, channel, &emote).await;
                    *pyramid_building = false;
                }
            }
            _ => *pyramid_building = false,
        }
    } else {
        *pyramid_building = false;
    }
}

async fn do_pyramid_action(combo: &ClientCombo, channel: &str, emote: &str) {
    let action: PyramidAction = rand::random();
    match action {
        PyramidAction::Steal => {
            say_rate_limited(combo, channel, emote.to_string()).await;
        }
        PyramidAction::Destroy => {
            say_rate_limited(combo, channel, "No".to_string()).await;
        }
        _ => warn!("Do nothing"),
    }
}

async fn do_auto_so(
    sender: &Sender<Command>,
    combo: &ClientCombo,
    msg: &twitch_irc::message::PrivmsgMessage,
    channel_conf: &ChannelConfig,
) {
    match &channel_conf.auto_so_channels {
        Some(auto_so_channels) if auto_so_channels.contains(&msg.sender.name) => {
            info!("{} {}", msg.channel_login, msg.sender.name);
            let (tx, rx) = oneshot::channel();
            let cmd = Command::GetSoStatus {
                channel: msg.channel_login.clone(),
                so_channel: msg.sender.name.clone(),
                resp: tx,
            };
            sender
                .send(cmd)
                .await
                .expect("Could not send request for so status");
            let already_sod = match rx.await {
                Ok(res) => res,
                Err(_) => true,
            };
            if !already_sod {
                say_rate_limited(
                    &combo,
                    &msg.channel_login,
                    format!("!so {}", msg.sender.name),
                )
                .await;
                let (tx, rx) = oneshot::channel();
                let cmd = Command::SetSoStatus {
                    channel: msg.channel_login.clone(),
                    so_channel: msg.sender.name.clone(),
                    val: true,
                    resp: tx,
                };
                sender
                    .send(cmd)
                    .await
                    .expect("Could not send request for so status");
                assert_eq!(rx.await.unwrap(), ())
            }
        }
        _ => {}
    }
}

async fn process_twitch_message(
    sender: &Sender<Command>,
    mut pyramids: &mut PyramidData,
    channel_confs: &HashMap<String, ChannelConfig>,
    combo: &ClientCombo,
    message: ServerMessage,
) {
    match message {
        ServerMessage::Privmsg(msg) => {
            info!(
                "(#{}) {}: {}",
                msg.channel_login, msg.sender.name, msg.message_text
            );

            let channel_conf = channel_confs.get(&msg.channel_login).unwrap();

            for action in channel_conf.permitted_actions.iter() {
                match action {
                    ChatAction::Ayy => {
                        do_ayy(&combo, &msg).await;
                    }
                    ChatAction::PyramidCounting => {
                        do_pyramid_counting(&combo, &msg, &sender).await;
                    }
                    ChatAction::PyramidInterference => {
                        do_pyramid_interference(&combo, &msg, &mut pyramids).await;
                    }

                    ChatAction::AutoSO => {
                        do_auto_so(&sender, &combo, &msg, &channel_conf).await;
                    }
                    _ => {}
                }
            }
        }
        ServerMessage::UserNotice(msg) => {
            let channel_conf = channel_confs.get(&msg.channel_login).unwrap();

            if channel_conf.permitted_actions.contains(&ChatAction::GiveSO) {
                if msg.event_id == "raid" {
                    say_rate_limited(
                        &combo,
                        &msg.channel_login,
                        format!("!so @{}", msg.sender.login),
                    )
                    .await;
                }
            }
            debug!("{:?}", msg)
        }
        _ => {
            debug!("{:?}", message)
        }
    }
}

pub fn message_loop(
    conf: &BotConfig,
    mut incoming_messages: UnboundedReceiver<ServerMessage>,
    cl: TwitchClient,
    sender: Sender<Command>,
) -> JoinHandle<()> {
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

    let channel_confs = conf.channels.iter().fold(HashMap::new(), |mut acc, c| {
        acc.insert(c.channel_name.clone(), c.clone());
        acc
    });
    let join_handle = tokio::spawn(async move {
        let lim = RateLimiter::keyed(
            Quota::with_period(Duration::from_secs_f32(1.5))
                .unwrap()
                .allow_burst(NonZeroU32::new(20).unwrap()),
        );

        let combo = ClientCombo {
            client: cl,
            limiter: lim,
        };
        info!("Starting chat");
        while let Some(message) = incoming_messages.recv().await {
            process_twitch_message(&sender, &mut pyramids, &channel_confs, &combo, message).await;
        }
    });
    join_handle
}
