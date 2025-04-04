use crate::bot_action::BotAction;
use regex::Regex;
use serde::Deserialize;
use std::collections::HashSet;

#[derive(Debug, Deserialize)]
pub struct BotConfig {
    pub name: String,
    pub credentials_file: String,
    pub client_secret: String,
    pub client_id: String,
    pub channels: Vec<ChannelConfig>,
    pub ntfy: Option<Ntfy>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct ChannelConfig {
    pub channel_name: String,
    pub permitted_actions: HashSet<BotAction>,
    pub auto_so_channels: Option<HashSet<String>>,
    pub harder_pyramids: Option<HashSet<String>>,
    pub easier_pyramids: Option<HashSet<String>>,
    pub automatic_responses: Option<Vec<RegexPair>>,
    pub prediction_monitoring: Option<bool>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct RegexPair {
    #[serde(with = "serde_regex")]
    pub regex: Regex,
    pub response: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Ntfy {
    pub address: String,
    pub user: String,
    pub pass: String,
}
