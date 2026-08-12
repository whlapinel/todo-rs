use crate::domain::team::TeamRole;

/// The one namespace every item belongs to — see docs/project-abstraction-plan.md.
/// `team_id` is `None` for a personal project (access = `owner_user_id` only) and
/// `Some` for a shared project (access = that team's active membership).
#[derive(Debug, Clone)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub owner_user_id: String,
    pub team_id: Option<String>,
}

/// A member's role/points within one specific project — reuses `TeamRole` rather
/// than introducing a near-identical enum; see the plan's "open call" about whether
/// `Team` itself keeps a role concept, which is still undecided as of stage A2.
#[derive(Debug, Clone)]
pub struct ProjectMember {
    pub project_id: String,
    pub user_id: String,
    pub role: TeamRole,
    pub points: i32,
}
