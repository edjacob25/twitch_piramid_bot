use serde::Deserialize;
use std::str::FromStr;

#[derive(Debug, Deserialize, Eq, PartialEq, Hash, Clone)]
pub enum BotAction {
    RespondSomething,
    PyramidCounting,
    PyramidInterference,
    GiveSO,
    AutoSO,
    CountBits,
    Queue,
}

impl FromStr for BotAction {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "RespondSomething" => Ok(BotAction::RespondSomething),
            "PyramidCounting" => Ok(BotAction::PyramidCounting),
            "PyramidInterference" => Ok(BotAction::PyramidInterference),
            "GiveSO" => Ok(BotAction::GiveSO),
            "AutoSO" => Ok(BotAction::AutoSO),
            "CountBits" => Ok(BotAction::CountBits),
            "Queue" => Ok(BotAction::Queue),
            _ => Err(()),
        }
    }
}
