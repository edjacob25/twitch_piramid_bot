use serde::Deserialize;
use std::str::FromStr;

#[derive(Debug, Deserialize, Eq, PartialEq, Hash, Clone)]
pub enum ChatAction {
    RespondSomething,
    PyramidCounting,
    PyramidInterference,
    GiveSO,
    AutoSO,
    CountBits,
    Queue,
}

impl FromStr for ChatAction {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "RespondSomething" => Ok(ChatAction::RespondSomething),
            "PyramidCounting" => Ok(ChatAction::PyramidCounting),
            "PyramidInterference" => Ok(ChatAction::PyramidInterference),
            "GiveSO" => Ok(ChatAction::GiveSO),
            "AutoSO" => Ok(ChatAction::AutoSO),
            "CountBits" => Ok(ChatAction::CountBits),
            "Queue" => Ok(ChatAction::Queue),
            _ => Err(()),
        }
    }
}
