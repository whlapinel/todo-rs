use crate::auth::AuthUser;
use crate::domain::team::TeamRole;
use super::nav::{self, ActiveContext, SidebarSection};
use crate::service::error::ItemError;
use crate::service::teams as team_service;
use crate::storage::sqlite::{ProjectRepo, TeamRepo, UserRepo};
use askama::Template;
use axum::extract::{Extension, Form, Path};
use axum::response::{Html, IntoResponse, Response};
use std::str::FromStr;
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
    nav_html: String,
}

async fn render_teams_page(
    teams: &Arc<dyn TeamRepo>,
    projects: &Arc<dyn ProjectRepo>,
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
    let nav_html =
        nav::build_nav_html(projects, user_id, ActiveContext::None, SidebarSection::None).await?;
    render(TeamsListPageTemplate {
        active_rows,
        pending_rows,
        nav_html,
    })
}

pub async fn teams_page(
    Extension(auth_user): Extension<AuthUser>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
) -> Result<Html<String>, ItemError> {
    render_teams_page(&teams, &projects, &auth_user.user_id).await
}

#[derive(serde::Deserialize)]
pub struct CreateTeamForm {
    name: String,
}

pub async fn create_team_form(
    Extension(auth_user): Extension<AuthUser>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Form(form): Form<CreateTeamForm>,
) -> Result<Html<String>, ItemError> {
    team_service::create_team(&teams, &projects, &form.name, &auth_user.user_id).await?;
    // A new team changes only the active-teams section, but accept/leave below also need to
    // touch the pending section — rendering the whole page keeps this handler consistent with
    // those, rather than being the one exception that returns a narrower fragment.
    render_teams_page(&teams, &projects, &auth_user.user_id).await
}

pub async fn accept_team_invite_form(
    Path(team_id): Path<String>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
) -> Result<Html<String>, ItemError> {
    team_service::accept_team_invite(&teams, &team_id, &auth_user.user_id).await?;
    render_teams_page(&teams, &projects, &auth_user.user_id).await
}

/// Also used for "Decline" on a pending invite — declining a team invite and leaving a team
/// you already belong to are the same underlying operation (remove this membership row),
/// matching the SPA's own `LeaveTeamCommand`-for-both behavior.
pub async fn leave_team_form(
    Path(team_id): Path<String>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
) -> Result<Response, ItemError> {
    team_service::leave_team(&teams, &team_id, &auth_user.user_id).await?;
    // Only reachable from the teams list page today (no "leave" affordance is wired into the
    // detail page's markup below yet, though the handler already supports being called from
    // there) — re-rendering the list in place covers the current call site.
    Ok(render_teams_page(&teams, &projects, &auth_user.user_id)
        .await?
        .into_response())
}

#[derive(Template)]
#[template(path = "teams/member_row.html")]
struct MemberRow {
    team_id: String,
    user_id: String,
    name: String,
    is_active: bool,
    is_role_admin: bool,
    /// Points are per-team (see CLAUDE.md) — this is the member's balance on *this*
    /// team specifically, not shown at all for a pending (non-`ACTIVE`) invite.
    points: Option<i32>,
    /// Whether the *viewer* can toggle this row's role — an active admin viewing
    /// another active member. Server-checked again in the handler itself, not just
    /// hidden here.
    show_role_toggle: bool,
    toggle_target_role: &'static str,
    toggle_label: &'static str,
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
    is_admin: bool,
    /// Stage B5e/B5f: retargets at the team's backing project's Tasks/Dashboard/Activity
    /// screens (`/web/projects/:project_id/...`) — every team has had a backing project since
    /// `ensure_team_project`/stage B1's backfill, but all three fall back to the `/web/projects`
    /// list page defensively if somehow none exists yet (the legacy per-type/team-dashboard/
    /// team-activity URLs these used to fall back to were removed in stage B5f).
    view_items_href: String,
    dashboard_href: String,
    activity_href: String,
    nav_html: String,
}

async fn render_team_detail(
    teams: &Arc<dyn TeamRepo>,
    users: &Arc<dyn UserRepo>,
    projects: &Arc<dyn ProjectRepo>,
    team_id: &str,
    requester_user_id: &str,
) -> Result<Html<String>, ItemError> {
    let team = team_service::get_team(teams, team_id).await?;
    let members = team_service::list_team_members(teams, team_id, requester_user_id).await?;

    let is_active_member = members
        .iter()
        .any(|m| m.user.id == requester_user_id && m.status == "ACTIVE");

    let viewer_is_admin = members.iter().any(|m| {
        m.user.id == requester_user_id && m.status == "ACTIVE" && m.role == TeamRole::Admin
    });

    let member_rows = members
        .iter()
        .map(|m| {
            let is_active = m.status == "ACTIVE";
            let is_role_admin = m.role == TeamRole::Admin;
            MemberRow {
                team_id: team_id.to_string(),
                user_id: m.user.id.clone(),
                name: format!("{} {}", m.user.first_name, m.user.last_name),
                is_active,
                is_role_admin,
                points: is_active.then_some(m.points),
                show_role_toggle: viewer_is_admin && is_active,
                toggle_target_role: if is_role_admin { "member" } else { "admin" },
                toggle_label: if is_role_admin { "Remove admin" } else { "Make admin" },
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

    let backing_project = projects.get_by_team(team_id).await.map_err(ItemError::from)?;
    let (view_items_href, dashboard_href, activity_href, active) = match &backing_project {
        Some(project) => (
            format!("/web/projects/{}/tasks", project.id),
            format!("/web/projects/{}/dashboard", project.id),
            format!("/web/projects/{}/activity", project.id),
            ActiveContext::Project(project.id.clone()),
        ),
        None => (
            "/web/projects".to_string(),
            "/web/projects".to_string(),
            "/web/projects".to_string(),
            ActiveContext::None,
        ),
    };

    let nav_html = nav::build_nav_html(
        projects,
        requester_user_id,
        active,
        SidebarSection::None,
    )
    .await?;

    render(TeamDetailPageTemplate {
        id: team.id,
        name: team.name,
        member_rows,
        invite_candidates,
        is_active_member,
        is_admin: viewer_is_admin,
        view_items_href,
        dashboard_href,
        activity_href,
        nav_html,
    })
}

pub async fn team_detail_page(
    Path(team_id): Path<String>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    Extension(users): Extension<Arc<dyn UserRepo>>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
) -> Result<Html<String>, ItemError> {
    render_team_detail(&teams, &users, &projects, &team_id, &auth_user.user_id).await
}

#[derive(serde::Deserialize)]
pub struct SetMemberRoleForm {
    role: String,
}

pub async fn set_team_member_role_form(
    Path((team_id, target_user_id)): Path<(String, String)>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    Extension(users): Extension<Arc<dyn UserRepo>>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Form(form): Form<SetMemberRoleForm>,
) -> Result<Html<String>, ItemError> {
    let new_role = TeamRole::from_str(&form.role)
        .map_err(|e| ItemError::Invalid(format!("invalid role: {e}")))?;
    team_service::set_team_member_role(
        &teams,
        &team_id,
        &auth_user.user_id,
        &target_user_id,
        new_role,
    )
    .await?;
    render_team_detail(&teams, &users, &projects, &team_id, &auth_user.user_id).await
}

#[derive(serde::Deserialize)]
pub struct UpdateTeamForm {
    name: String,
}

pub async fn update_team_form(
    Path(team_id): Path<String>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(teams): Extension<Arc<dyn TeamRepo>>,
    Extension(users): Extension<Arc<dyn UserRepo>>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Form(form): Form<UpdateTeamForm>,
) -> Result<Html<String>, ItemError> {
    team_service::update_team(&teams, &team_id, &auth_user.user_id, &form.name).await?;
    render_team_detail(&teams, &users, &projects, &team_id, &auth_user.user_id).await
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
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
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
    render_team_detail(&teams, &users, &projects, &team_id, &auth_user.user_id).await
}
