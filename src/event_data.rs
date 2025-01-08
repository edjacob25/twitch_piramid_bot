use crate::twitch_ws::{Condition, Transport};
use futures_util::stream::{SplitSink, SplitStream};
use serde::{Deserialize, Serialize};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::{Bytes, Message};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

pub type WSWriter = SplitSink<WebSocketStream<MaybeTlsStream<TcpStream>>, Message>;
pub type WSReader = SplitStream<WebSocketStream<MaybeTlsStream<TcpStream>>>;

#[derive(Debug, Serialize)]
pub struct EventRequestBody {
    #[serde(rename = "type")]
    pub sub_type: String,
    pub version: String,
    pub condition: Condition,
    pub transport: Transport,
}

#[derive(Debug, Deserialize)]
pub struct UsersResponse {
    pub data: Vec<UserData>,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub struct UserData {
    pub id: String,
    login: String,
    pub display_name: String,
    #[serde(rename = "type")]
    user_type: String,
    broadcaster_type: String,
    description: String,
    profile_image_url: String,
    offline_image_url: String,
    view_count: u32,
    created_at: String,
}

pub enum MessageResponse {
    ConnectionSuccessful(Vec<String>),
    Continue,
    Reconnect(String),
    Pong(Bytes),
    Close,
}

pub struct EventData<'a> {
    pub event: &'a str,
    pub broadcaster_id: &'a str,
    pub session_id: &'a str,
    pub name: &'a str,
}
