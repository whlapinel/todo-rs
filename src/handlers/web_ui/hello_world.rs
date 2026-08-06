use askama::Template;
use axum::http::StatusCode;
use axum::response::Html;

#[derive(Template)]
#[template(path = "hello.html")]
struct HelloTemplate {
    name: String,
}

pub async fn hello_world() -> Result<Html<String>, StatusCode> {
    let template = HelloTemplate {
        name: "world".to_string(),
    };

    template
        .render()
        .map(Html)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}
