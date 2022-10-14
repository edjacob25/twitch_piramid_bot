use crate::chat_action::ChatAction;
use serde::Deserialize;
use std::collections::HashSet;

#[derive(Debug, Deserialize)]
pub struct BotConfig {
    pub name: String,
    pub oauth_token: String,
    pub channels: Vec<ChannelConfig>,
}

#[derive(Debug, Deserialize)]
pub struct ChannelConfig {
    pub channel_name: String,
    pub permitted_actions: HashSet<ChatAction>,
}
