use serde::Deserialize;
use std::str::FromStr;

#[derive(Debug, Deserialize, Eq, PartialEq, Hash, Clone)]
pub enum ChatAction {
    Ayy,
    PyramidCounting,
    PyramidInterference,
    GiveSO,
}

impl FromStr for ChatAction {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "Ayy" => Ok(ChatAction::Ayy),
            "PyramidCounting" => Ok(ChatAction::PyramidCounting),
            "PyramidInterference" => Ok(ChatAction::PyramidInterference),

            _ => Err(()),
        }
    }
}
