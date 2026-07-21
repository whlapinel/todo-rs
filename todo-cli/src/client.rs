use todo_client::config::Token;

pub fn build_client(url: &str, token: &str) -> todo_client::Client {
    let config = todo_client::Config::builder()
        .endpoint_url(format!("{}/api", url.trim_end_matches('/')))
        .behavior_version_latest()
        .bearer_token(Token::new(token.to_string(), None))
        .build();
    todo_client::Client::from_conf(config)
}
