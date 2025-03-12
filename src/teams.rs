use rusqlite::ToSql;
use rusqlite::types::ToSqlOutput;
use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};
use std::str::FromStr;

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum Status {
    Confirmed,
    Unconfirmed,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Member {
    pub name: String,
    pub status: Status,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Team {
    pub members: Vec<Member>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Queue {
    pub size: u8,
    pub team_size: u8,
    pub teams: Vec<Team>,
}

impl Display for Team {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "Equipo con ")?;
        for (count, member) in self.members.iter().enumerate() {
            if count != 0 {
                write!(f, ", ")?;
            }
            if member.status == Status::Unconfirmed {
                write!(f, "{}(no confirmado)", member.name)?;
                continue;
            }
            write!(f, "{}", member.name)?;
        }
        Ok(())
    }
}

impl Display for Queue {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "| ")?;
        for team in &self.teams {
            write!(f, "{} | ", team)?;
        }
        Ok(())
    }
}

impl ToSql for Team {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        Ok(self.to_string().into())
    }
}

impl FromStr for Status {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "Confirmed" => Ok(Status::Confirmed),
            "Unconfirmed" => Ok(Status::Unconfirmed),
            _ => Err(()),
        }
    }
}
