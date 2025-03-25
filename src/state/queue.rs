use super::data::Status::{Confirmed, Unconfirmed};
use super::data::{AddResult, ConfirmResult, DeletionResult, Member, MoveResult, Queue, Team};
use super::{DB_NAME, Source, StateManager};
use anyhow::bail;
use log::info;
use rusqlite::{Connection, params};

impl StateManager {
    pub fn create_queue(&self, channel: &str, teams: u8, per_team: u8) -> anyhow::Result<()> {
        info!("Creating queue for channel {channel} with {teams} teams and {per_team} spaces per team");
        let conn = Connection::open(DB_NAME)?;
        let teams_vec = (0..teams).map(|_| Team::default()).collect::<Vec<_>>();
        let json = serde_json::to_string(&teams_vec)?;
        if let Err(e) = conn.execute(
            "INSERT INTO queue VALUES (?1, ?2, ?3, $4, $5) ON CONFLICT(channel) DO UPDATE SET no_teams=?2, team_size=?3, teams=$4, active=$5",
            params![channel, teams, per_team, json, true],
        ) {
            bail!("Db error when creating queue: {}", e);
        };
        let q = Queue {
            size: teams,
            team_size: per_team,
            teams: teams_vec,
            active: true,
        };
        self.sender.send((q, channel.to_string())).ok();
        Ok(())
    }

    pub fn get_queue(channel: &str) -> anyhow::Result<Queue> {
        info!("Getting queue for channel {channel}");
        let conn = Connection::open(DB_NAME)?;
        conn.query_row_and_then(
            "SELECT no_teams, team_size, teams, active FROM queue WHERE channel = ?1",
            [channel],
            |row| {
                let json: String = row.get(2)?;
                let teams: Vec<Team> = serde_json::from_str(&json)?;
                Ok(Queue {
                    size: row.get(0)?,
                    team_size: row.get(1)?,
                    active: row.get(3)?,
                    teams,
                })
            },
        )
    }

    fn update_queue(&self, channel: &str, queue: Queue, operation: &str) -> anyhow::Result<()> {
        let conn = Connection::open(DB_NAME)?;
        let json = serde_json::to_string(&queue.teams)?;
        if let Err(e) = conn.execute(
            "UPDATE queue SET teams = json(?2), active=?3 WHERE channel = ?1",
            params![channel, json, queue.active],
        ) {
            bail!("Db error when {operation} user to channel {channel}: {e}");
        };
        self.sender.send((queue, channel.to_string())).ok();
        Ok(())
    }

    pub fn add_to_queue(
        &self,
        channel: &str,
        user: String,
        second_user: Option<String>,
        pref_team: Option<u8>,
        source: Source,
    ) -> anyhow::Result<AddResult> {
        info!("Adding {user} to channel {channel}");
        let mut queue = Self::get_queue(channel)?;
        if source == Source::Chat && !queue.active {
            return Ok(AddResult::QueueFrozen);
        }

        let mut users = 2u8;
        let second_user = second_user.unwrap_or_else(|| {
            users = 1;
            String::new()
        });

        let mut free_spaces = vec![];
        let mut already_found = false;
        for team in &queue.teams {
            free_spaces.push(queue.team_size - team.members.len() as u8);
            let names = team.members.iter().map(|x| x.name.as_str()).collect::<Vec<_>>();
            if names.contains(&user.as_str()) || (users == 2 && names.contains(&second_user.as_str())) {
                already_found = true;
            }
        }
        if already_found {
            return Ok(AddResult::AlreadyInQueue);
        }

        let mut chosen_idx = None;
        if let Some(preferred_team) = pref_team {
            let real_idx = preferred_team as usize;
            if real_idx < free_spaces.len() && free_spaces[real_idx] >= users {
                chosen_idx = Some(real_idx);
            }
        }

        if chosen_idx.is_none() {
            for (idx, team_free_space) in free_spaces.iter().enumerate() {
                if *team_free_space >= users {
                    chosen_idx = Some(idx);
                    break;
                }
            }
        }
        if let Some(chosen_idx) = chosen_idx {
            queue.teams[chosen_idx].members.push(Member {
                name: user,
                status: Confirmed,
            });
            if users == 2 {
                queue.teams[chosen_idx].members.push(Member {
                    name: second_user.to_string(),
                    status: Unconfirmed,
                });
            }
            self.update_queue(channel, queue, "Adding")?;
            Ok(AddResult::Success(chosen_idx))
        } else {
            Ok(AddResult::NoSpace)
        }
    }

    pub fn confirm_user(&self, channel: &str, user: &str) -> anyhow::Result<ConfirmResult> {
        info!("Confirming {user} in channel {channel}");
        let mut queue = Self::get_queue(channel)?;
        let mut found = false;
        let mut idx = 0;
        for team in &mut queue.teams {
            for member in &mut team.members {
                if member.name == user && member.status == Unconfirmed {
                    member.status = Confirmed;
                    found = true;
                    break;
                }
            }
            idx += 1;
        }
        if found {
            self.update_queue(channel, queue, "confirming")?;
            Ok(ConfirmResult::Success(idx))
        } else {
            Ok(ConfirmResult::NotFound)
        }
    }

    pub fn move_to_other_team(
        &self,
        channel: &str,
        user: &str,
        desired_team: u8,
        source: Source,
    ) -> anyhow::Result<MoveResult> {
        info!("Moving {user} to team {desired_team} in channel {channel}");
        let mut queue = Self::get_queue(channel)?;
        if source == Source::Chat && !queue.active {
            return Ok(MoveResult::QueueFrozen);
        }
        if desired_team as usize >= queue.teams.len() {
            return Ok(MoveResult::InvalidTeam);
        }
        let available_space = queue.teams[desired_team as usize].members.len() < queue.team_size as usize;
        if !available_space {
            return Ok(MoveResult::NoSpace);
        }
        let mut final_idx = None;
        for (t_idx, team) in queue.teams.iter().enumerate() {
            for (idx, member) in team.members.iter().enumerate() {
                if member.name == user {
                    final_idx = Some((t_idx, idx));
                    break;
                }
            }
        }

        if let Some(final_idx) = final_idx {
            if final_idx.0 == desired_team as usize {
                return Ok(MoveResult::AlreadyInTeam);
            }
            let p = queue.teams[final_idx.0].members.remove(final_idx.1);
            queue.teams[desired_team as usize].members.push(p);

            self.update_queue(channel, queue, "moving")?;
            Ok(MoveResult::Success)
        } else {
            Ok(MoveResult::NotFound)
        }
    }

    pub fn delete_from_queue(&self, channel: &str, user: &str, source: Source) -> anyhow::Result<DeletionResult> {
        info!("Deleting {user} in channel {channel}");
        let mut queue = Self::get_queue(channel)?;
        if source == Source::Chat && !queue.active {
            return Ok(DeletionResult::QueueFrozen);
        }
        let mut found = false;
        for team in &mut queue.teams {
            for (idx, member) in team.members.iter().enumerate() {
                if member.name == user {
                    team.members.remove(idx);
                    found = true;
                    break;
                }
            }
        }

        if found {
            self.update_queue(channel, queue, "deleting")?;
            Ok(DeletionResult::Success)
        } else {
            Ok(DeletionResult::NotFound)
        }
    }

    pub fn switch_queue(&self, channel: &str) -> anyhow::Result<bool> {
        info!("Freezing/unfreezing queue for channel {channel}");
        let mut queue = Self::get_queue(channel)?;
        let result = !queue.active;
        queue.active = result;
        self.update_queue(channel, queue, "freezing/unfreezing")?;
        Ok(result)
    }
}
