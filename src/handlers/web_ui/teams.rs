use crate::auth::AuthUser;
use crate::service::error::ItemError;
use crate::service::teams as team_service;
use crate::storage::sqlite::{TeamRepo, UserRepo};
use askama::Template;
use axum::extract::{Extension, Form, Path};
use axum::response::{Html, IntoResponse, Response};
use std::sync::Arc;

fn render<T: Template>(t: T) -> Result<Html<String>, ItemError> {
    Ok(Html(t.render()?))
}

#[derive(Template)]
#[template(path = "teams/active_row.html")]
struct ActiveTeamRow {
    id: String,
    name: String,
}

#[derive(Template)]
#[template(path = "teams/pending_row.html")]
struct PendingTeamRow {
    id: String,
    name: String,
    invited_by_name: Option<String>,
}

#[derive(Template)]
#[template(path = "teams/list_page.html")]
struct TeamsListPageTemplate {
    active_rows: Vec<String>,
    pending_rows: Vec<String>,
}

async fn render_teams_page(
    teams: &Arc<dyn TeamRepo>,
    user_id: &str,
) -> Result<Html<String>, ItemError> {
    let memberships = team_service::list_teams(teams, user_id).await?;
    let mut active_rows = Vec::new();
    let mut pending_rows = Vec::new();
    for m in memberships {
        if m.status == "ACTIVE" {
            active_rows.push(
                ActiveTeamRow {
                    id: m.team.id,
                    name: m.team.name,
                }
                .render()?,
            );
        } else {
            pending_rows.push(
                PendingTeamRow {
                    id: m.team.id,
                    name: m.team.name,
                    invited_by_name: m.invited_by_name,
                }
                .render()?,
            );
        }
    }
    render(TeamsListPageTemplate {
        active_rows,
        pending_rows,
    })
}

pub async fn teams_page(
    Extension(auth_user): Extension<AuthUser>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
) -> Result<Html<String>, ItemError> {
    render_teams_page(&teams, &auth_user.user_id).await
}

#[derive(serde::Deserialize)]
pub struct CreateTeamForm {
    name: String,
}

pub async fn create_team_form(
    Extension(auth_user): Extension<AuthUser>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    Form(form): Form<CreateTeamForm>,
) -> Result<Html<String>, ItemError> {
    team_service::create_team(&teams, &form.name, &auth_user.user_id).await?;
    // A new team changes only the active-teams section, but accept/leave below also need to
    // touch the pending section — rendering the whole page keeps this handler consistent with
    // those, rather than being the one exception that returns a narrower fragment.
    render_teams_page(&teams, &auth_user.user_id).await
}

pub async fn accept_team_invite_form(
    Path(team_id): Path<String>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
) -> Result<Html<String>, ItemError> {
    team_service::accept_team_invite(&teams, &team_id, &auth_user.user_id).await?;
    render_teams_page(&teams, &auth_user.user_id).await
}

/// Also used for "Decline" on a pending invite — declining a team invite and leaving a team
/// you already belong to are the same underlying operation (remove this membership row),
/// matching the SPA's own `LeaveTeamCommand`-for-both behavior.
pub async fn leave_team_form(
    Path(team_id): Path<String>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
) -> Result<Response, ItemError> {
    team_service::leave_team(&teams, &team_id, &auth_user.user_id).await?;
    // Only reachable from the teams list page today (no "leave" affordance is wired into the
    // detail page's markup below yet, though the handler already supports being called from
    // there) — re-rendering the list in place covers the current call site.
    Ok(render_teams_page(&teams, &auth_user.user_id)
        .await?
        .into_response())
}

#[derive(Template)]
#[template(path = "teams/member_row.html")]
struct MemberRow {
    user_id: String,
    name: String,
    is_active: bool,
}

#[derive(Template)]
#[template(path = "teams/detail_page.html")]
struct TeamDetailPageTemplate {
    id: String,
    name: String,
    member_rows: Vec<String>,
    /// (user_id, display name) for every user not already on this team, in any status —
    /// candidates for a fresh invite.
    invite_candidates: Vec<(String, String)>,
    is_active_member: bool,
}

async fn render_team_detail(
    teams: &Arc<dyn TeamRepo>,
    users: &Arc<dyn UserRepo>,
    team_id: &str,
    requester_user_id: &str,
) -> Result<Html<String>, ItemError> {
    let team = team_service::get_team(teams, team_id).await?;
    let members = team_service::list_team_members(teams, team_id, requester_user_id).await?;

    let is_active_member = members
        .iter()
        .any(|m| m.user.id == requester_user_id && m.status == "ACTIVE");

    let member_rows = members
        .iter()
        .map(|m| {
            MemberRow {
                user_id: m.user.id.clone(),
                name: format!("{} {}", m.user.first_name, m.user.last_name),
                is_active: m.status == "ACTIVE",
            }
            .render()
        })
        .collect::<Result<Vec<_>, _>>()?;

    let all_users = users.list().await.map_err(ItemError::from)?;
    let invite_candidates = all_users
        .into_iter()
        .filter(|u| !members.iter().any(|m| m.user.id == u.id))
        .map(|u| (u.id, format!("{} {}", u.first_name, u.last_name)))
        .collect();

    render(TeamDetailPageTemplate {
        id: team.id,
        name: team.name,
        member_rows,
        invite_candidates,
        is_active_member,
    })
}

pub async fn team_detail_page(
    Path(team_id): Path<String>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    Extension(users): Extension<Arc<dyn UserRepo>>,
) -> Result<Html<String>, ItemError> {
    render_team_detail(&teams, &users, &team_id, &auth_user.user_id).await
}

#[derive(serde::Deserialize)]
pub struct InviteForm {
    #[serde(rename = "inviteeUserId")]
    invitee_user_id: String,
}

pub async fn invite_team_member_form(
    Path(team_id): Path<String>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    Extension(users): Extension<Arc<dyn UserRepo>>,
    Form(form): Form<InviteForm>,
) -> Result<Html<String>, ItemError> {
    team_service::invite_team_member(
        &teams,
        &users,
        &team_id,
        &auth_user.user_id,
        &form.invitee_user_id,
    )
    .await?;
    // The member list and the invite-candidate dropdown both change together (the invitee
    // moves from one to the other), so — same reasoning as create_team_form above — this
    // re-renders the whole detail page rather than a narrower fragment.
    render_team_detail(&teams, &users, &team_id, &auth_user.user_id).await
}
