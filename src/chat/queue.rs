use crate::chat::ChatLoop;
use crate::state::Source;
use crate::state::command::Command;
use anyhow::{Result, bail};
use tokio::sync::oneshot;
use twitch_irc::message::PrivmsgMessage as ChatMessage;

impl ChatLoop {
    pub async fn handle_queue(&mut self, msg: &ChatMessage) {
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

    fn sanitize_username(user: &str) -> String {
        user.replace('@', "").to_lowercase()
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
            return (Some(Self::sanitize_username(split[1])), None);
        }

        let preferred_team = split[2].parse::<u8>().ok();
        (Some(Self::sanitize_username(split[1])), preferred_team)
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
        use crate::state::data::AddResult::*;
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
        use crate::state::data::ConfirmResult::*;
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
        use crate::state::data::DeletionResult::*;
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
        Ok((preferred_team, Some(Self::sanitize_username(split[1]))))
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
        use crate::state::data::MoveResult::*;
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
}
