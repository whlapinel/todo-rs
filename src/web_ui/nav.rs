use crate::service::error::ItemError;
use crate::service::projects as project_service;
use crate::storage::sqlite::ProjectRepo;
use askama::Template;
use std::sync::Arc;

/// Which project (if any) the current page is scoped to. Derived purely from the request's
/// URL by each handler (`/web/projects/:project_id/...` routes vs. pages with no single
/// natural project, like `/web/projects` itself, `/web/assigned-items`, or the teams list) —
/// there is deliberately no persisted "active project" cookie/session, same precedent the
/// old Personal/Team `ActiveContext` set.
#[derive(Clone, PartialEq)]
pub enum ActiveContext {
    Project(String),
    /// The cross-project Tasks/Events/Calendar screens (`/web/tasks`, `/web/events`,
    /// `/web/calendar`) — the all-projects counterpart to `Project(id)`, with its own fixed
    /// (not `section_href`-derived) sidebar section links. See `build_nav_html`.
    AllProjects,
    None,
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
    ItemSeries,
    None,
}

struct ProjectPill {
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
    project_pills: Vec<ProjectPill>,
    section_links: Vec<SectionLink>,
    calendar_href: Option<String>,
    activity_href: Option<String>,
}

/// The one place that knows how a given (section, project) pair maps to a URL — used both
/// for the sidebar's 4 section links (project fixed at the page's own active project, section
/// varies over Tasks/Events/SimpleLists/Templates) and the top switcher's project pills
/// (section fixed at the page's own `section`, project varies over every project the user
/// belongs to). That shared derivation is what makes "switching project preserves the current
/// section" fall out for free rather than needing special-case code — same property the
/// Personal/Team switcher this replaces already had.
///
/// `SidebarSection::None` maps to that project's Calendar — the closest analog of the old
/// Personal/Team switcher's "no single section" fallback.
fn section_href(section: SidebarSection, project_id: &str) -> String {
    let path = match section {
        SidebarSection::Tasks => "tasks",
        SidebarSection::Events => "events",
        SidebarSection::SimpleLists => "simple-lists",
        SidebarSection::Templates => "templates",
        SidebarSection::ItemSeries => "series",
        SidebarSection::None => "calendar",
    };
    format!("/web/projects/{project_id}/{path}")
}

pub async fn build_nav_html(
    projects: &Arc<dyn ProjectRepo>,
    user_id: &str,
    active: ActiveContext,
    section: SidebarSection,
) -> Result<String, ItemError> {
    let project_pills = project_service::list_projects(projects, user_id)
        .await?
        .into_iter()
        .map(|p| {
            let id = p.id.clone();
            ProjectPill {
                is_active: active == ActiveContext::Project(id.clone()),
                href: section_href(section, &id),
                name: p.name,
            }
        })
        .collect();

    // The 4 section links and the Calendar/Activity fixed links only make sense once a
    // project is actually active — a page like the projects list or the teams list has no
    // single project to scope them to, so they're simply omitted there rather than pointing
    // at an arbitrary project.
    let (section_links, calendar_href, activity_href) = match &active {
        ActiveContext::Project(project_id) => {
            let links = [
                (SidebarSection::Tasks, "Tasks"),
                (SidebarSection::Events, "Events"),
                (SidebarSection::SimpleLists, "Simple Lists"),
                (SidebarSection::Templates, "Templates"),
                (SidebarSection::ItemSeries, "Series"),
            ]
            .into_iter()
            .map(|(s, label)| SectionLink {
                label,
                href: section_href(s, project_id),
                is_active: s == section,
            })
            .collect();
            (
                links,
                Some(section_href(SidebarSection::None, project_id)),
                Some(format!("/web/projects/{project_id}/activity")),
            )
        }
        ActiveContext::AllProjects => {
            let links = [
                (SidebarSection::Tasks, "Tasks", "/web/tasks"),
                (SidebarSection::Events, "Events", "/web/events"),
            ]
            .into_iter()
            .map(|(s, label, href)| SectionLink {
                label,
                href: href.to_string(),
                is_active: s == section,
            })
            .collect();
            (links, Some("/web/calendar".to_string()), None)
        }
        ActiveContext::None => (Vec::new(), None, None),
    };

    Ok(NavTemplate {
        project_pills,
        section_links,
        calendar_href,
        activity_href,
    }
    .render()?)
}
