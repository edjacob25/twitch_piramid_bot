use std::str::FromStr;
use serde::Deserialize;

#[derive(Debug, Deserialize, Eq, PartialEq, Hash)]
pub enum ChatAction {
    DoPyramid,
    Ayy,
}

impl FromStr for ChatAction {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "DoPyramid" => Ok(ChatAction::DoPyramid),
            "Ayy" => Ok(ChatAction::Ayy),
            _ => Err(())
        }
    }
}