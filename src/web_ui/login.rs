use askama::Template;
use axum::http::StatusCode;
use axum::response::Html;

#[derive(Template)]
#[template(path = "login.html")]
struct LoginTemplate;

pub async fn login_page() -> Result<Html<String>, StatusCode> {
    LoginTemplate
        .render()
        .map(Html)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}
