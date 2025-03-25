use crate::bot_action::BotAction;
use crate::bot_config::{BotConfig, ChannelConfig};
use crate::bot_token_storage::CustomTokenStorage;
use crate::state::{Command, Source};
use anyhow::{Result, bail};
use governor::Quota;
use governor::clock::DefaultClock;
use governor::state::RateLimiter;
use governor::state::keyed::DefaultKeyedStateStore;
use log::{debug, info, warn};
use pyramid_action::PyramidAction;
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

pub mod pyramid_action;

type TwitchClient = TwitchIRCClient<SecureWSTransport, RefreshingLoginCredentials<CustomTokenStorage>>;
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
    tokio::spawn(async move {
        info!("Creating chat loop");
        ChatLoop::new(conf, incoming_messages, cl, sender).run().await;
    })
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
        for channel_to_connect in &conf.channels {
            cl.join(channel_to_connect.channel_name.clone())
                .expect("Could not connect to a channel");
        }

        let mut pyramids = PyramidData {
            building_flags: HashMap::new(),
            emote_counts: HashMap::new(),
            emotes: HashMap::new(),
        };

        for channel_to_connect in &conf.channels {
            let channel_name = &channel_to_connect.channel_name;
            pyramids.building_flags.insert(channel_name.clone(), false);
            pyramids.emote_counts.insert(channel_name.clone(), 0usize);
            pyramids.emotes.insert(channel_name.clone(), String::new());
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
            Ok(()) => self.cl.say(channel, msg).await.unwrap(),
            Err(_) => {
                warn!("Rate limited");
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
            PyramidAction::DoNothing => warn!("Do nothing"),
        }
    }

    async fn respond_something(&self, msg: &ChatMessage) {
        let config = self.channel_configs.get(msg.channel_login.as_str()).unwrap();

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
        let is_online = rx.await.unwrap_or(false);

        if !is_online {
            return;
        }

        let as_vec = msg.message_text.split(' ').collect::<Vec<_>>();
        let name = as_vec[as_vec.len() - 2];

        let (tx, rx) = oneshot::channel();
        let cmd = Command::IncrementPyramid {
            channel: msg.channel_login.clone(),
            user: name.to_string(),
            resp: tx,
        };
        let _ = self.sender.send(cmd).await;
        let num = rx.await.unwrap_or(0);
        let message = format!("{name} lleva {num} piramides");
        self.say_rate_limited(msg.channel_login.as_str(), message).await;
    }

    async fn do_pyramid_interference(&mut self, msg: &ChatMessage) {
        let channel = msg.channel_login.as_str();
        let config = self.channel_configs.get(channel).unwrap();

        let pyramid_building = self.pyramids.building_flags.get_mut(channel).unwrap();
        let emote_count = self.pyramids.emote_counts.get_mut(channel).unwrap();
        let emote = self.pyramids.emotes.get_mut(channel).unwrap();

        if !msg.message_text.contains(' ') {
            *pyramid_building = true;
            *emote_count = 1;
            emote.clone_from(&msg.message_text);
            info!("Single word {}", *emote);
            return;
        }
        if !*pyramid_building {
            return;
        }
        let emote = emote.clone();
        let num_of_matches = msg.message_text.match_indices(&emote).collect::<Vec<_>>().len();
        let num_of_words = msg.message_text.split(' ').collect::<Vec<_>>().len();
        if num_of_words != num_of_matches {
            *pyramid_building = false;
            return;
        }
        match num_of_matches {
            i if i == *emote_count + 1 => {
                info!("Pyramid growing");
                *emote_count += 1;

                if let Some(h) = config.harder_pyramids.as_ref() {
                    if h.contains(&msg.sender.name) && *emote_count == 3 && rand::random::<f32>() < 0.5 {
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
                let already_sod = rx.await.unwrap_or(true);
                if !already_sod {
                    self.say_rate_limited(&msg.channel_login, format!("!so {}", msg.sender.name))
                        .await;
                    let cmd = Command::SetSoStatus {
                        channel: msg.channel_login.clone(),
                        so_channel: msg.sender.name.clone(),
                        val: true,
                    };
                    self.sender
                        .send(cmd)
                        .await
                        .expect("Could not send request for so status");
                }
            }
            _ => {}
        }
    }

    async fn count_bits(&self, msg: &ChatMessage) {
        if let Some(bits) = msg.bits {
            let cmd = Command::CountBits {
                channel: msg.channel_login.clone(),
                user: msg.sender.login.clone(),
                bits,
            };
            let _ = self.sender.send(cmd).await;
        }
    }

    async fn handle_queue(&mut self, msg: &ChatMessage) {
        let channel = &msg.channel_login;
        let user = msg.sender.login.clone();
        let _admin = Self::check_admin(msg);
        match msg.message_text.as_str().trim() {
            s if s.starts_with("!crear") => self.create_queue(&user, channel, s).await,
            s if s.starts_with("!entrar") => self.join_queue(user, channel, s).await,
            s if s.starts_with("!borrar") => self.admin_remove(channel, &user, s).await,
            s if s.starts_with("!mover") => self.move_user(channel, user, s).await,
            s if s.starts_with("!llamar") => self.call_team(channel, s).await,
            "!salir" => self.delete_user(channel, user).await,
            "!confirmar" => self.confirm_user(channel, user).await,
            "!equipos" => self.show_queue(channel).await,
            _ => {}
        }
    }

    fn check_admin(msg: &ChatMessage) -> bool {
        let badges = msg.badges.iter().map(|b| b.name.to_lowercase()).collect::<Vec<_>>();
        badges.iter().any(|x| x.eq("broadcaster") || x.eq("moderator"))
    }
    fn parse_create_opts(msg: &str) -> Result<(u8, u8)> {
        let split = msg.trim().split(' ').collect::<Vec<_>>();
        if split.len() < 3 {
            bail!("Invalid number of options {}", split.len());
        }
        let num_teams = split[1].parse::<u8>()?;
        let num_persons = split[2].parse::<u8>()?;
        Ok((num_teams, num_persons))
    }

    async fn create_queue(&self, user: &str, channel: &str, msg: &str) {
        if user != channel {
            return;
        }
        if let Ok((teams, per_team)) = Self::parse_create_opts(msg) {
            let _ = self
                .sender
                .send(Command::CreateQueue {
                    channel: channel.to_string(),
                    teams,
                    per_team,
                })
                .await;
            self.say_rate_limited(
                channel,
                format!("Creados {teams} equipos, con {per_team} jugadores cada uno"),
            )
            .await;
            return;
        }
        self.say_rate_limited(
            channel,
            "Error al llamar el comando !crear, prueba con algo como '!crear 3 3' ".to_string(),
        )
        .await;
    }

    fn parse_join_opts(msg: &str) -> (Option<String>, Option<u8>) {
        let split = msg.trim().split(' ').collect::<Vec<_>>();
        let len = split.len();
        if len == 1 {
            return (None, None);
        }
        if len == 2 {
            if let Ok(num) = split[1].parse::<u8>() {
                return (None, Some(num));
            }
            return (Some(split[1].replace('@', "").to_string()), None);
        }

        let preferred_team = split[2].parse::<u8>().ok();
        (Some(split[1].replace('@', "").to_string()), preferred_team)
    }

    async fn join_queue(&self, user: String, channel: &str, msg: &str) {
        let (extra, team) = Self::parse_join_opts(msg);
        if let Some(t) = team {
            if t == 0 {
                self.say_rate_limited(channel, "No se pudo anotar para el equipo 0".to_string())
                    .await;
                return;
            }
        }
        let team = team.map(|team| team - 1);
        let (tx, rx) = oneshot::channel();
        let cmd = Command::AddToQueue {
            channel: channel.to_string(),
            user: user.clone(),
            second_user: extra.clone(),
            team,
            source: Source::Chat,
            resp: tx,
        };
        let _ = self.sender.send(cmd).await;
        use crate::teams::AddResult::*;
        let result = rx.await.unwrap_or(GeneralError);
        match result {
            Success(t) => {
                self.say_rate_limited(channel, format!("Anotado(s) en el equipo {}", t + 1))
                    .await;
            }
            AlreadyInQueue => {
                let comp = if extra.is_some() {
                    format!("{user} or {}", extra.unwrap())
                } else {
                    user
                };
                self.say_rate_limited(channel, format!("No se pudo agregar, {comp} ya esta en algún equipo"))
                    .await;
            }
            NoSpace => {
                self.say_rate_limited(channel, "No se pudo agregar, no hay lugares suficientes".to_string())
                    .await;
            }
            GeneralError => self.say_rate_limited(channel, "No se pudo anotar".to_string()).await,
            QueueFrozen => {
                self.say_rate_limited(channel, "No se puede agregar. La lista esta congelada".to_string())
                    .await;
            }
        }
    }

    async fn show_queue(&self, channel: &str) {
        let (tx, rx) = oneshot::channel();
        let _ = self
            .sender
            .send(Command::ShowQueue {
                channel: channel.to_string(),
                resp: tx,
            })
            .await;
        if let Ok(queue) = rx.await {
            self.say_rate_limited(channel, format!("{queue}")).await;
        } else {
            self.say_rate_limited(channel, "No se pueden mostrar equipos por el momento".to_string())
                .await;
        }
    }

    async fn confirm_user(&self, channel: &str, user: String) {
        let (tx, rx) = oneshot::channel();
        let cmd = Command::ConfirmUser {
            channel: channel.to_string(),
            user: user.clone(),
            resp: tx,
        };
        let _ = self.sender.send(cmd).await;
        use crate::teams::ConfirmResult::*;
        match rx.await.unwrap_or(GeneralError) {
            Success(i) => {
                self.say_rate_limited(channel, format!("{user} confirmado en equipo {}", i + 1))
                    .await;
            }
            NotFound => {
                self.say_rate_limited(channel, format!("{user} no encontrado para confirmar"))
                    .await;
            }
            GeneralError => self.say_rate_limited(channel, "No se pudo confirmar".to_string()).await,
        }
    }

    fn parse_remove_opts(msg: &str) -> Option<String> {
        let split = msg.trim().split(' ').collect::<Vec<_>>();
        let len = split.len();
        if len > 1 {
            return Some(split[1].replace('@', "").to_string());
        }
        None
    }

    async fn admin_remove(&self, channel: &str, login: &str, msg: &str) {
        if channel != login {
            self.say_rate_limited(channel, "Tu no puedes borrar a alguien mas".to_string())
                .await;
            return;
        }
        if let Some(target) = Self::parse_remove_opts(msg) {
            self.delete_user(channel, target).await;
        } else {
            self.say_rate_limited(channel, "No hay a quien borrar".to_string())
                .await;
        }
    }

    async fn delete_user(&self, channel: &str, user: String) {
        let (tx, rx) = oneshot::channel();
        let cmd = Command::RemoveFromQueue {
            channel: channel.to_string(),
            user: user.clone(),
            source: Source::Chat,
            resp: tx,
        };
        let _ = self.sender.send(cmd).await;
        use crate::teams::DeletionResult::*;
        match rx.await.unwrap_or(GeneralError) {
            Success => {
                self.say_rate_limited(channel, format!("{user} ha sido borrado de la cola"))
                    .await;
            }
            NotFound => {
                self.say_rate_limited(channel, format!("{user} no esta en la cola"))
                    .await;
            }
            GeneralError => {
                self.say_rate_limited(channel, format!("Ha habido un error borrando {user}"))
                    .await;
            }
            QueueFrozen => {
                self.say_rate_limited(channel, "No se puede borrar. La lista esta congelada".to_string())
                    .await;
            }
        }
    }

    fn parse_move_opts(msg: &str) -> Result<(u8, Option<String>)> {
        let split = msg.trim().split(' ').collect::<Vec<_>>();
        let len = split.len();
        if len == 1 {
            bail!("No team selected");
        }
        if len == 2 {
            return Ok((split[1].parse::<u8>()?, None));
        }

        let preferred_team = split[2].parse::<u8>()?;
        Ok((preferred_team, Some(split[1].replace('@', "").to_string())))
    }

    async fn move_user(&self, channel: &str, user: String, msg: &str) {
        let (team, target) = if let Ok(res) = Self::parse_move_opts(msg) {
            (res.0, res.1.unwrap_or(user.clone()))
        } else {
            self.say_rate_limited(channel, "Hubo un error con las opciones del comando".to_string())
                .await;
            return;
        };
        if user != target && user != channel {
            self.say_rate_limited(channel, "Tu no puedes mover personas".to_string())
                .await;
            return;
        }

        if team == 0 {
            self.say_rate_limited(channel, "No se pudo mover para el equipo 0".to_string())
                .await;
            return;
        }
        let (tx, rx) = oneshot::channel();
        let cmd = Command::MoveToOtherTeam {
            channel: channel.to_string(),
            user: target.clone(),
            team: team - 1,
            source: Source::Chat,
            resp: tx,
        };
        let _ = self.sender.send(cmd).await;
        use crate::teams::MoveResult::*;
        match rx.await.unwrap_or(GeneralError) {
            Success => {
                self.say_rate_limited(channel, format!("{target} ha sido movido al equipo {team}"))
                    .await;
            }
            NotFound => {
                self.say_rate_limited(channel, format!("{target} no esta en ningun equipo"))
                    .await;
            }
            NoSpace => {
                self.say_rate_limited(channel, "No hay espacio en el equipo deseado".to_string())
                    .await;
            }
            InvalidTeam => {
                self.say_rate_limited(channel, "No existe ese equipo".to_string()).await;
            }
            AlreadyInTeam => {
                self.say_rate_limited(channel, format!("{target} ya esta en al equipo {team}"))
                    .await;
            }
            GeneralError => {
                self.say_rate_limited(channel, format!("No se pudo mover {target} al equipo {team}"))
                    .await;
            }
            QueueFrozen => {
                self.say_rate_limited(channel, "No se puede mover. La lista esta congelada".to_string())
                    .await;
            }
        }
    }

    fn parse_call_opts(msg: &str) -> Result<usize> {
        let split = msg.trim().split(' ').collect::<Vec<_>>();
        let len = split.len();
        if len == 1 {
            bail!("No team selected");
        }
        Ok(split[1].parse::<usize>()?)
    }

    async fn call_team(&self, channel: &str, msg: &str) {
        let Ok(team) = Self::parse_call_opts(msg) else {
            self.say_rate_limited(channel, "Error con las opciones del comando".to_string())
                .await;
            return;
        };
        let (tx, rx) = oneshot::channel();

        let _ = self
            .sender
            .send(Command::ShowQueue {
                channel: channel.to_string(),
                resp: tx,
            })
            .await;

        if let Ok(queue) = rx.await {
            if team < 1 || team > queue.teams.len() {
                self.say_rate_limited(channel, "Ese equipo no existe".to_string()).await;
                return;
            }
            let mut msg = "Llamando a ".to_string();

            if let Some(m) = queue.teams[team - 1].members.first() {
                msg.push_str(&format!("@{}", m.name));
            }
            for member in queue.teams[team - 1].members.iter().skip(1) {
                msg.push_str(&format!(", @{}", member.name));
            }
            msg.push_str(&format!(" para el equipo {team}"));
            self.say_rate_limited(channel, msg).await;
        } else {
            self.say_rate_limited(channel, "No se pueden llamar por el momento".to_string())
                .await;
        }
    }

    async fn process_twitch_message(&mut self, message: ServerMessage) {
        match message {
            ServerMessage::Privmsg(msg) => {
                info!("(#{}) {}: {}", msg.channel_login, msg.sender.name, msg.message_text);

                let actions = self
                    .channel_configs
                    .get(&msg.channel_login)
                    .unwrap()
                    .permitted_actions
                    .clone();

                for action in &actions {
                    match action {
                        BotAction::RespondSomething => {
                            self.respond_something(&msg).await;
                        }
                        BotAction::PyramidCounting => {
                            self.do_pyramid_counting(&msg).await;
                        }
                        BotAction::PyramidInterference => {
                            self.do_pyramid_interference(&msg).await;
                        }
                        BotAction::AutoSO => {
                            self.do_auto_so(&msg).await;
                        }
                        BotAction::CountBits => {
                            self.count_bits(&msg).await;
                        }
                        BotAction::Queue => {
                            self.handle_queue(&msg).await;
                        }
                        BotAction::GiveSO => {}
                    }
                }
            }
            ServerMessage::UserNotice(msg) => {
                let channel_conf = self.channel_configs.get(&msg.channel_login).unwrap();

                if !channel_conf.permitted_actions.contains(&BotAction::GiveSO) {
                    return;
                }
                if msg.event_id != "raid" {
                    return;
                }
                self.say_rate_limited(&msg.channel_login, format!("!so @{}", msg.sender.login))
                    .await;
                debug!("{:?}", msg);
            }
            _ => {
                debug!("{:?}", message);
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
