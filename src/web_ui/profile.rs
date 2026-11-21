use crate::auth::AuthUser;
use crate::domain::user::User;
use crate::service::error::ItemError;
use crate::service::users::{self, ProfileUpdate};
use crate::storage::sqlite::{ProjectRepo, UserRepo};
use crate::web_ui::nav::{self, ActiveContext, SidebarSection};
use askama::Template;
use axum::extract::{Extension, Form};
use axum::response::Html;
use std::sync::Arc;

#[derive(Template)]
#[template(path = "profile/page.html")]
pub struct ProfilePageTemplate {
    pub first_name: String,
    pub last_name: String,
    pub email: String,
    /// Empty string when unset, so the template's "Not set" option matches it.
    pub timezone: String,
    pub timezones: Vec<&'static str>,
    pub saved: bool,
    pub nav_html: String,
}

async fn render_profile_page(
    projects: &Arc<dyn ProjectRepo>,
    user: User,
    saved: bool,
) -> Result<Html<String>, ItemError> {
    let nav_html = nav::build_nav_html(
        projects,
        &user.id,
        ActiveContext::None,
        SidebarSection::None,
    )
    .await?;
    let page = ProfilePageTemplate {
        first_name: user.first_name,
        last_name: user.last_name,
        email: user.email.unwrap_or_default(),
        timezone: user.timezone.unwrap_or_default(),
        timezones: chrono_tz::TZ_VARIANTS.iter().map(|tz| tz.name()).collect(),
        saved,
        nav_html,
    };
    Ok(Html(page.render()?))
}

/// The signed-in user's own profile. Its main reason to exist is `timezone`: background
/// work with no live request (push reminder text, calendar sync's all-day dates) can't read
/// the browser's `X-Tz-Offset-Minutes` header and uses this stored zone instead. Before this
/// screen it could only be set through the JSON API, so it went stale after a move.
pub async fn profile_page(
    Extension(auth_user): Extension<AuthUser>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(users): Extension<Arc<dyn UserRepo>>,
) -> Result<Html<String>, ItemError> {
    let user = users.get(&auth_user.user_id).await?;
    render_profile_page(&projects, user, false).await
}

#[derive(serde::Deserialize)]
pub struct ProfileForm {
    #[serde(rename = "firstName")]
    first_name: String,
    #[serde(rename = "lastName", default)]
    last_name: String,
    #[serde(default)]
    timezone: String,
}

pub async fn update_profile_form(
    Extension(auth_user): Extension<AuthUser>,
    Extension(projects): Extension<Arc<dyn ProjectRepo>>,
    Extension(users): Extension<Arc<dyn UserRepo>>,
    Form(form): Form<ProfileForm>,
) -> Result<Html<String>, ItemError> {
    let user = users::update_profile(
        &users,
        &auth_user.user_id,
        ProfileUpdate {
            first_name: form.first_name,
            last_name: form.last_name,
            timezone: Some(form.timezone),
        },
    )
    .await?;
    render_profile_page(&projects, user, true).await
}
