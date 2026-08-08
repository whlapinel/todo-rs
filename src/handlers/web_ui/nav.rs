use crate::service::error::ItemError;
use crate::service::teams as team_service;
use crate::storage::sqlite::TeamRepo;
use askama::Template;
use std::sync::Arc;

/// Which "profile" the current page is scoped to. Derived purely from the request's URL by
/// each handler (personal routes vs. `/team-items/:team_id/...` routes) — there is
/// deliberately no persisted "active team" cookie/session (see the nav plan's locked-in
/// decisions): pages with no natural single-team scope (Dashboard, Assigned to me, the
/// Teams list) just pass `Personal`.
#[derive(Clone, PartialEq)]
pub enum ActiveContext {
    Personal,
    Team(String),
}

/// Which item-type screen (if any) the current page belongs to. Drives both the sidebar's
/// active-link highlighting and, via `section_href`, the href every nav link/pill actually
/// points at — see that function's doc comment.
#[derive(Clone, Copy, PartialEq)]
pub enum SidebarSection {
    Tasks,
    Events,
    SimpleLists,
    Templates,
    None,
}

impl SidebarSection {
    /// Maps the interim `?kind=` query param (see items.rs/team_items.rs) to a section.
    /// Only task/simple/event are reachable this way — Template-typed items never appear in
    /// a generic items/team-items listing at all (excluded at the storage layer), so there is
    /// no `"template"` case here.
    pub fn from_kind(kind: Option<&str>) -> Self {
        match kind {
            Some("task") => SidebarSection::Tasks,
            Some("simple") => SidebarSection::SimpleLists,
            Some("event") => SidebarSection::Events,
            _ => SidebarSection::None,
        }
    }
}

struct TeamPill {
    name: String,
    href: String,
    is_active: bool,
}

struct SectionLink {
    label: &'static str,
    href: String,
    is_active: bool,
}

#[derive(Template)]
#[template(path = "nav.html")]
struct NavTemplate {
    me_href: String,
    is_personal_active: bool,
    team_pills: Vec<TeamPill>,
    section_links: Vec<SectionLink>,
}

/// The one place that knows how a given (section, context) pair maps to a URL — used both
/// for the sidebar's 4 section links (context fixed at the page's own `active`, section
/// varies over Tasks/Events/SimpleLists/Templates) and the top switcher's Me/Team pills
/// (section fixed at the page's own `section`, context varies over Personal/each team).
/// That shared derivation is what makes "switching team preserves the current section" (a
/// locked-in product decision) fall out for free rather than needing special-case code.
///
/// Every section now has a fully dedicated screen in both contexts, so no `?kind=` interim
/// filter is used by this function anymore (the generic `/team-items`/`/items` catch-alls
/// themselves are untouched and still support it directly, just no longer linked to here).
/// Templates was the last holdout — team-scoped templates (Stage 8) added
/// `/team-templates/:team_id`, so Templates/Team(id) no longer falls back to the personal
/// `/templates` screen either.
fn section_href(section: SidebarSection, ctx: &ActiveContext) -> String {
    match (section, ctx) {
        (SidebarSection::Tasks, ActiveContext::Personal) => "/web/tasks".to_string(),
        (SidebarSection::Tasks, ActiveContext::Team(id)) => {
            format!("/web/team-tasks/{id}")
        }
        (SidebarSection::Events, ActiveContext::Personal) => "/web/events".to_string(),
        (SidebarSection::Events, ActiveContext::Team(id)) => {
            format!("/web/team-events/{id}")
        }
        (SidebarSection::SimpleLists, ActiveContext::Personal) => {
            "/web/simple-lists".to_string()
        }
        (SidebarSection::SimpleLists, ActiveContext::Team(id)) => {
            format!("/web/team-simple-lists/{id}")
        }
        (SidebarSection::Templates, ActiveContext::Personal) => "/web/templates".to_string(),
        (SidebarSection::Templates, ActiveContext::Team(id)) => {
            format!("/web/team-templates/{id}")
        }
        (SidebarSection::None, ActiveContext::Personal) => "/web/items".to_string(),
        (SidebarSection::None, ActiveContext::Team(id)) => format!("/web/team-items/{id}"),
    }
}

pub async fn build_nav_html(
    team_repo: &Arc<dyn TeamRepo>,
    user_id: &str,
    active: ActiveContext,
    section: SidebarSection,
) -> Result<String, ItemError> {
    let team_pills = team_service::list_teams(team_repo, user_id)
        .await?
        .into_iter()
        .filter(|t| t.status == "ACTIVE")
        .map(|t| {
            let id = t.team.id;
            TeamPill {
                is_active: active == ActiveContext::Team(id.clone()),
                href: section_href(section, &ActiveContext::Team(id)),
                name: t.team.name,
            }
        })
        .collect();

    let section_links = [
        (SidebarSection::Tasks, "Tasks"),
        (SidebarSection::Events, "Events"),
        (SidebarSection::SimpleLists, "Simple Lists"),
        (SidebarSection::Templates, "Templates"),
    ]
    .into_iter()
    .map(|(s, label)| SectionLink {
        label,
        href: section_href(s, &active),
        is_active: s == section,
    })
    .collect();

    Ok(NavTemplate {
        me_href: section_href(section, &ActiveContext::Personal),
        is_personal_active: active == ActiveContext::Personal,
        team_pills,
        section_links,
    }
    .render()?)
}
