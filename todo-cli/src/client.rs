use todo_client::config::interceptors::BeforeTransmitInterceptorContextMut;
use todo_client::config::{ConfigBag, Intercept, RuntimeComponents};
use todo_client::error::BoxError;

#[derive(Debug)]
struct AuthHeaderInterceptor {
    token: String,
}

impl Intercept for AuthHeaderInterceptor {
    fn name(&self) -> &'static str {
        "AuthHeaderInterceptor"
    }

    fn modify_before_transmit(
        &self,
        context: &mut BeforeTransmitInterceptorContextMut<'_>,
        _runtime_components: &RuntimeComponents,
        _cfg: &mut ConfigBag,
    ) -> Result<(), BoxError> {
        context
            .request_mut()
            .headers_mut()
            .insert("authorization", format!("Bearer {}", self.token));
        Ok(())
    }
}

pub fn build_client(url: &str, token: &str) -> todo_client::Client {
    let config = todo_client::Config::builder()
        .endpoint_url(format!("{}/api", url.trim_end_matches('/')))
        .behavior_version_latest()
        .interceptor(AuthHeaderInterceptor {
            token: token.to_string(),
        })
        .build();
    todo_client::Client::from_conf(config)
}
