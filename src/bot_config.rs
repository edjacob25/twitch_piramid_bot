use serde::Deserialize;
use std::collections::HashSet;
use crate::chat_action::ChatAction;

#[derive(Debug, Deserialize)]
pub struct BotConfig {
    pub name: String,
    pub oauth_token: String,
    pub channels: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct ChannelConfig {
    pub channel_name: String,
    pub permitted_actions: HashSet<ChatAction>,
}
