use crate::chat_action::ChatAction;
use serde::Deserialize;
use std::collections::HashSet;

#[derive(Debug, Deserialize)]
pub struct BotConfig {
    pub name: String,
    pub oauth_token: String,
    pub client_id: String,
    pub channels: Vec<ChannelConfig>,
    pub ntfy: Option<Ntfy>
}

#[derive(Debug, Deserialize, Clone)]
pub struct ChannelConfig {
    pub channel_name: String,
    pub permitted_actions: HashSet<ChatAction>,
    pub auto_so_channels: Option<HashSet<String>>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Ntfy {
    pub address: String,
    pub user: String,
    pub pass: String
}