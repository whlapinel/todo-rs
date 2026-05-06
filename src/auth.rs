use std::sync::Arc;

use axum::{
    body::Body,
    extract::Query,
    http::{self, Request, StatusCode},
    middleware::Next,
    response::{IntoResponse, Redirect, Response},
    Extension, Json,
};
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use oauth2::{
    basic::BasicClient, reqwest::async_http_client, AuthUrl, AuthorizationCode, ClientId,
    ClientSecret, CsrfToken, PkceCodeChallenge, PkceCodeVerifier, RedirectUrl, Scope, TokenResponse,
    TokenUrl,
};
use serde::{Deserialize, Serialize};
use tower_cookies::{Cookie, Cookies};

use crate::storage::{RepoError, UserRepo};

#[derive(Clone)]
pub struct AppState {
    pub oauth_client: BasicClient,
    pub jwt_secret: String,
    pub user_repo: Arc<dyn UserRepo>,
}

impl AppState {
    pub fn new(
        google_client_id: String,
        google_client_secret: String,
        base_url: String,
        jwt_secret: String,
        user_repo: Arc<dyn UserRepo>,
    ) -> Self {
        let oauth_client = BasicClient::new(
            ClientId::new(google_client_id),
            Some(ClientSecret::new(google_client_secret)),
            AuthUrl::new("https://accounts.google.com/o/oauth2/v2/auth".to_string()).unwrap(),
            Some(
                TokenUrl::new("https://www.googleapis.com/oauth2/v4/token".to_string()).unwrap(),
            ),
        )
        .set_redirect_uri(RedirectUrl::new(format!("{base_url}/auth/callback")).unwrap());

        Self { oauth_client, jwt_secret, user_repo }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct Claims {
    sub: String,
    exp: usize,
}

#[derive(Debug, Clone)]
pub struct AuthUser {
    pub user_id: String,
}

pub async fn auth_login(
    Extension(state): Extension<Arc<AppState>>,
    cookies: Cookies,
) -> impl IntoResponse {
    let (pkce_challenge, pkce_verifier) = PkceCodeChallenge::new_random_sha256();
    let (auth_url, csrf_token) = state
        .oauth_client
        .authorize_url(CsrfToken::new_random)
        .add_scope(Scope::new("openid".to_string()))
        .add_scope(Scope::new("email".to_string()))
        .add_scope(Scope::new("profile".to_string()))
        .set_pkce_challenge(pkce_challenge)
        .url();

    let mut state_cookie = Cookie::new("oauth_state", csrf_token.secret().clone());
    state_cookie.set_http_only(true);
    state_cookie.set_path("/");
    cookies.add(state_cookie);

    let mut pkce_cookie = Cookie::new("oauth_pkce_verifier", pkce_verifier.secret().clone());
    pkce_cookie.set_http_only(true);
    pkce_cookie.set_path("/");
    cookies.add(pkce_cookie);

    Redirect::to(auth_url.as_str())
}

#[derive(Deserialize)]
pub struct CallbackParams {
    code: String,
    state: String,
}

#[derive(Deserialize)]
struct GoogleUserInfo {
    sub: String,
    email: String,
    given_name: String,
    family_name: Option<String>,
}

pub async fn auth_callback(
    Extension(state): Extension<Arc<AppState>>,
    Query(params): Query<CallbackParams>,
    cookies: Cookies,
) -> Response {
    let stored_state = match cookies.get("oauth_state") {
        Some(c) => c.value().to_string(),
        None => return (StatusCode::BAD_REQUEST, "Missing state cookie").into_response(),
    };
    if stored_state != params.state {
        return (StatusCode::BAD_REQUEST, "State mismatch").into_response();
    }

    let pkce_secret = match cookies.get("oauth_pkce_verifier") {
        Some(c) => c.value().to_string(),
        None => return (StatusCode::BAD_REQUEST, "Missing PKCE cookie").into_response(),
    };
    let pkce_verifier = PkceCodeVerifier::new(pkce_secret);

    cookies.remove(Cookie::build("oauth_state", "").path("/").finish());
    cookies.remove(Cookie::build("oauth_pkce_verifier", "").path("/").finish());

    let token = match state
        .oauth_client
        .exchange_code(AuthorizationCode::new(params.code))
        .set_pkce_verifier(pkce_verifier)
        .request_async(async_http_client)
        .await
    {
        Ok(t) => t,
        Err(e) => {
            tracing::error!("Token exchange failed: {e:?}");
            return (StatusCode::INTERNAL_SERVER_ERROR, "Token exchange failed").into_response();
        }
    };

    let access_token = token.access_token().secret().to_string();
    let userinfo: GoogleUserInfo = match reqwest::Client::new()
        .get("https://www.googleapis.com/oauth2/v3/userinfo")
        .bearer_auth(&access_token)
        .send()
        .await
        .and_then(|r| r.error_for_status())
    {
        Ok(r) => match r.json().await {
            Ok(u) => u,
            Err(e) => {
                tracing::error!("Failed to parse user info: {e}");
                return (StatusCode::INTERNAL_SERVER_ERROR, "Failed to parse user info")
                    .into_response();
            }
        },
        Err(e) => {
            tracing::error!("Failed to fetch user info: {e}");
            return (StatusCode::INTERNAL_SERVER_ERROR, "Failed to fetch user info")
                .into_response();
        }
    };

    let user = match state
        .user_repo
        .get_or_create_by_google_id(
            &userinfo.sub,
            &userinfo.email,
            &userinfo.given_name,
            userinfo.family_name.as_deref().unwrap_or(""),
        )
        .await
    {
        Ok(u) => u,
        Err(e) => {
            tracing::error!("DB error in get_or_create_by_google_id: {e:?}");
            return (StatusCode::INTERNAL_SERVER_ERROR, "Database error").into_response();
        }
    };

    let exp = (chrono::Utc::now() + chrono::Duration::days(7)).timestamp() as usize;
    let claims = Claims { sub: user.id.clone(), exp };
    let jwt = match encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(state.jwt_secret.as_bytes()),
    ) {
        Ok(t) => t,
        Err(e) => {
            tracing::error!("JWT encode error: {e}");
            return (StatusCode::INTERNAL_SERVER_ERROR, "Failed to create session")
                .into_response();
        }
    };

    let mut auth_cookie = Cookie::new("todo_auth", jwt);
    auth_cookie.set_http_only(true);
    auth_cookie.set_path("/");
    cookies.add(auth_cookie);

    Redirect::to(&format!("/users/{}", user.id)).into_response()
}

pub async fn auth_logout(cookies: Cookies) -> impl IntoResponse {
    cookies.remove(Cookie::build("todo_auth", "").path("/").finish());
    Redirect::to("/")
}

pub async fn auth_me(
    Extension(state): Extension<Arc<AppState>>,
    cookies: Cookies,
) -> Response {
    let token = match cookies.get("todo_auth").map(|c| c.value().to_string()) {
        Some(t) => t,
        None => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({"error": "not authenticated"})),
            )
                .into_response()
        }
    };

    let claims = match decode::<Claims>(
        &token,
        &DecodingKey::from_secret(state.jwt_secret.as_bytes()),
        &Validation::default(),
    ) {
        Ok(d) => d.claims,
        Err(_) => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({"error": "invalid token"})),
            )
                .into_response()
        }
    };

    match state.user_repo.get(&claims.sub).await {
        Ok(user) => Json(serde_json::json!({
            "userId": user.id,
            "firstName": user.first_name,
            "lastName": user.last_name,
        }))
        .into_response(),
        Err(RepoError::NotFound) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "user not found"})),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": format!("{e:?}")})),
        )
            .into_response(),
    }
}

pub async fn jwt_auth_middleware(
    Extension(state): Extension<Arc<AppState>>,
    mut req: Request<Body>,
    next: Next<Body>,
) -> Response {
    let token = req
        .headers()
        .get(http::header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| {
            s.split(';').find_map(|part| {
                let part = part.trim();
                part.strip_prefix("todo_auth=").map(|v| v.to_string())
            })
        });

    match token {
        Some(t) => match decode::<Claims>(
            &t,
            &DecodingKey::from_secret(state.jwt_secret.as_bytes()),
            &Validation::default(),
        ) {
            Ok(data) => {
                req.extensions_mut().insert(AuthUser { user_id: data.claims.sub });
                next.run(req).await
            }
            Err(_) => (
                StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({"error": "invalid or expired token"})),
            )
                .into_response(),
        },
        None => (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({"error": "authentication required"})),
        )
            .into_response(),
    }
}
