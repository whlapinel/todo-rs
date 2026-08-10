use std::fmt;
use std::str::FromStr;

#[derive(Debug, Clone)]
pub struct Team {
    pub id: String,
    pub name: String,
}

/// A member's standing within one specific team — never global (see CLAUDE.md's
/// per-team roles/points design). A user can be `Admin` of one team and `Member`
/// of another.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum TeamRole {
    Admin,
    #[default]
    Member,
}

impl TeamRole {
    pub fn as_str(&self) -> &'static str {
        match self {
            TeamRole::Admin => "admin",
            TeamRole::Member => "member",
        }
    }
}

impl fmt::Display for TeamRole {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for TeamRole {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "admin" => Ok(TeamRole::Admin),
            "member" => Ok(TeamRole::Member),
            other => Err(format!("unknown team role: {other}")),
        }
    }
}

#[derive(Debug, Clone)]
pub struct TeamMember {
    pub team_id: String,
    pub user_id: String,
    pub status: String,
    pub invited_by: Option<String>,
    pub role: TeamRole,
}

impl TeamMember {
    pub fn is_admin(&self) -> bool {
        self.role == TeamRole::Admin
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn team_role_round_trips_through_as_str() {
        for role in [TeamRole::Admin, TeamRole::Member] {
            assert_eq!(TeamRole::from_str(role.as_str()), Ok(role));
        }
    }

    #[test]
    fn team_role_rejects_unknown_string() {
        assert!(TeamRole::from_str("owner").is_err());
    }

    #[test]
    fn team_role_defaults_to_member() {
        assert_eq!(TeamRole::default(), TeamRole::Member);
    }
}
