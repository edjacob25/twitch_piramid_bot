use super::data::{AddResult, ConfirmResult, DeletionResult, MoveResult, Queue};
use super::{Responder, Source};
use crate::events::twitch_ws::Event;

#[derive(Debug)]
pub enum Command {
    GetChannelStatus {
        key: String,
        resp: Responder<bool>,
    },
    SetChannelStatus {
        key: String,
        val: bool,
    },
    GetSoStatus {
        channel: String,
        so_channel: String,
        resp: Responder<bool>,
    },
    SetSoStatus {
        channel: String,
        so_channel: String,
        val: bool,
    },
    ResetSoStatus {
        channel: String,
    },
    StartPrediction {
        channel: String,
        question: String,
    },
    PredictionProgress {
        channel: String,
        responses: Vec<(String, Vec<(String, u32)>)>,
    },
    PredictionEnd {
        channel: String,
    },
    GetStreamInfo {
        channel: String,
        resp: Responder<Event>,
    },
    SetStreamInfo {
        channel: String,
        event: Box<Event>,
    },
    CountBits {
        channel: String,
        user: String,
        bits: u64,
    },
    IncrementPyramid {
        channel: String,
        user: String,
        resp: Responder<i32>,
    },
    CreateQueue {
        channel: String,
        teams: u8,
        per_team: u8,
    },
    ResetQueue {
        channel: String,
    },
    AddToQueue {
        channel: String,
        user: String,
        second_user: Option<String>,
        team: Option<u8>,
        source: Source,
        resp: Responder<AddResult>,
    },
    ConfirmUser {
        channel: String,
        user: String,
        resp: Responder<ConfirmResult>,
    },
    RemoveFromQueue {
        channel: String,
        user: String,
        source: Source,
        resp: Responder<DeletionResult>,
    },
    MoveToOtherTeam {
        channel: String,
        user: String,
        team: u8,
        source: Source,
        resp: Responder<MoveResult>,
    },
    ShowQueue {
        channel: String,
        resp: Responder<Queue>,
    },
    SwitchQueueStatus {
        channel: String,
        resp: Responder<anyhow::Result<bool>>,
    },
}
