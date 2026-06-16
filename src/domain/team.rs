#[derive(Debug, Clone)]
pub struct Team {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone)]
pub struct TeamMember {
    pub team_id: String,
    pub user_id: String,
    pub status: String,
    pub invited_by: Option<String>,
}
