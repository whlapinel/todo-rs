use lettre::{
    AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor, message::header::ContentType,
    transport::smtp::authentication::Credentials,
};

pub struct EmailConfig {
    pub smtp_user: String,
    pub smtp_password: String,
    pub smtp_url: String,
    pub base_url: String,
}

impl EmailConfig {
    pub fn from_env() -> Option<Self> {
        let smtp_user = std::env::var("TODO_SMTP_USER").ok()?;
        let smtp_password = std::env::var("TODO_SMTP_PASSWORD").ok()?;
        let smtp_url = std::env::var("TODO_SMTP_URL").ok()?;
        let base_url = std::env::var("TODO_BASE_URL").ok()?;
        Some(Self {
            smtp_user,
            smtp_password,
            smtp_url,
            base_url,
        })
    }
}

pub async fn send_app_invite(
    config: &EmailConfig,
    to_email: &str,
    inviter_name: &str,
) -> Result<(), String> {
    let body = format!(
        "{inviter_name} has invited you to join the People's Republic of Lists.\n\nSign in here: {}",
        config.base_url
    );

    // Display name is the inviter; sending address is always the SMTP account.
    let from = format!("{inviter_name} <{}>", config.smtp_user)
        .parse()
        .map_err(|e| format!("invalid from address: {e}"))?;
    let to = to_email
        .parse()
        .map_err(|e| format!("invalid to address: {e}"))?;

    let email = Message::builder()
        .from(from)
        .to(to)
        .subject("You've been invited to join the People's Republic of Lists")
        .header(ContentType::TEXT_PLAIN)
        .body(body)
        .map_err(|e| format!("failed to build email: {e}"))?;

    let creds = Credentials::new(config.smtp_user.clone(), config.smtp_password.clone());
    let mailer = AsyncSmtpTransport::<Tokio1Executor>::relay(&config.smtp_url)
        .map_err(|e| format!("SMTP relay error: {e}"))?
        .credentials(creds)
        .build();

    mailer
        .send(email)
        .await
        .map_err(|e| format!("SMTP send error: {e}"))?;
    Ok(())
}
