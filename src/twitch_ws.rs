#![allow(dead_code)]

use serde;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(from = "String")]
pub enum MessageType {
    Welcome,
    KeepAlive,
    Notification,
    Reconnect,
    Revocation,
}

impl From<String> for MessageType {
    fn from(value: String) -> Self {
        match value.as_str() {
            "session_welcome" => MessageType::Welcome,
            "session_keepalive" => MessageType::KeepAlive,
            "notification" => MessageType::Notification,
            "session_reconnect" => MessageType::Reconnect,
            "revocation" => MessageType::Revocation,
            _ => MessageType::KeepAlive,
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct Subscription {
    pub id: String,
    pub status: String,
    #[serde(rename = "type")]
    pub sub_type: String,
    pub version: String,
    pub cost: u32,
    pub condition: Condition,
    pub transport: Transport,
    pub created_at: String,
}

#[derive(Debug, Deserialize)]
pub struct Condition {
    pub broadcaster_user_id: String,
}

#[derive(Debug, Deserialize)]
pub struct Transport {
    pub method: String,
    pub session_id: String,
}

#[derive(Debug, Deserialize)]
pub struct Event {
    pub id: Option<String>,
    pub broadcaster_user_id: String,
    pub broadcaster_user_login: String,
    pub broadcaster_user_name: String,
    #[serde(rename = "type")]
    pub online_type: Option<String>,
    pub started_at: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct Session {
    pub id: String,
    pub status: String,
    pub connected_at: String,
    pub keepalive_timeout_seconds: Option<u8>,
    pub reconnect_url: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct Metadata {
    pub message_id: String,
    pub message_type: MessageType,
    pub message_timestamp: String,
    pub subscription_type: Option<String>,
    pub subscription_version: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct Payload {
    session: Option<Session>,
    subscription: Option<Subscription>,
    event: Option<Event>,
}

#[derive(Debug, Deserialize)]
pub struct GeneralMessage {
    pub metadata: Metadata,
    pub payload: Payload,
}

#[derive(Debug, Deserialize)]
pub struct WelcomeMessage {
    pub session: Session,
}

impl From<Payload> for WelcomeMessage {
    fn from(a: Payload) -> Self {
        WelcomeMessage {
            session: a.session.unwrap(),
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct KeepAliveMessage {}

#[derive(Debug, Deserialize)]
pub struct NotificationMessage {
    pub subscription: Subscription,
    pub event: Event,
}

impl From<Payload> for NotificationMessage {
    fn from(a: Payload) -> Self {
        NotificationMessage {
            subscription: a.subscription.unwrap(),
            event: a.event.unwrap(),
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct ReconnectMessage {
    pub session: Session,
}

impl From<Payload> for ReconnectMessage {
    fn from(a: Payload) -> Self {
        ReconnectMessage {
            session: a.session.unwrap(),
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct RevocationMessage {
    pub subscription: Subscription,
}
impl From<Payload> for RevocationMessage {
    fn from(a: Payload) -> Self {
        RevocationMessage {
            subscription: a.subscription.unwrap(),
        }
    }
}
