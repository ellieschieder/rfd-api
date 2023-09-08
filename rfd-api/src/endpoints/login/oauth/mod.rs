use async_trait::async_trait;
use dropshot::Method;
use http::{header, Request};
use hyper::{body::to_bytes, client::connect::Connect, Body, Client};
use oauth2::{basic::BasicClient, url::ParseError, AuthUrl, ClientId, ClientSecret, TokenUrl};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::fmt::{Debug, Display};
use thiserror::Error;
use tracing::instrument;

use super::{UserInfo, UserInfoError, UserInfoProvider};

pub mod authz_code;
pub mod client;
pub mod device_token;
pub mod google;

#[derive(Debug, Error)]
pub enum OAuthProviderError {
    #[error("Unable to instantiate invalid provider")]
    FailToCreateInvalidProvider,
}

pub trait OAuthProvider: ExtractUserInfo + Debug {
    fn name(&self) -> OAuthProviderName;
    fn scopes(&self) -> Vec<&str>;
    fn client_id(&self) -> &str;
    fn client_secret(&self) -> Option<&str>;
    fn user_info_endpoint(&self) -> &str;
    fn device_code_endpoint(&self) -> &str;
    fn auth_url_endpoint(&self) -> &str;
    fn token_exchange_content_type(&self) -> &str;
    fn token_exchange_endpoint(&self) -> &str;

    fn provider_info(&self, public_url: &str) -> OAuthProviderInfo {
        OAuthProviderInfo {
            provider: self.name(),
            client_id: self.client_id().to_string(),
            auth_url_endpoint: self.auth_url_endpoint().to_string(),
            device_code_endpoint: self.device_code_endpoint().to_string(),
            token_endpoint: format!("{}/login/oauth/{}/device/exchange", public_url, self.name(),),
            scopes: self
                .scopes()
                .into_iter()
                .map(|s| s.to_string())
                .collect::<Vec<_>>(),
        }
    }

    fn as_client(&self) -> Result<BasicClient, ParseError> {
        Ok(BasicClient::new(
            ClientId::new(self.client_id().to_string()),
            self.client_secret()
                .map(|s| ClientSecret::new(s.to_string())),
            AuthUrl::new(self.auth_url_endpoint().to_string())?,
            Some(TokenUrl::new(self.token_exchange_endpoint().to_string())?),
        ))
    }
}

pub trait ExtractUserInfo {
    fn extract_user_info(&self, data: &[u8]) -> Result<UserInfo, UserInfoError>;
}

// Trait describing an factory function for constructing an OAuthProvider
pub trait OAuthProviderFn: Fn() -> Box<dyn OAuthProvider + Send + Sync> + Send + Sync {}
impl<T> OAuthProviderFn for T where T: Fn() -> Box<dyn OAuthProvider + Send + Sync> + Send + Sync {}

// Add a blanket implementation of the user information extractor for all OAuth providers. This
// handles the common calling code to the provider's user information calling code and then
// delegates the deserialization/information extraction to the provider.
#[async_trait]
impl<T> UserInfoProvider for T
where
    T: OAuthProvider + ExtractUserInfo + Send + Sync + ?Sized,
{
    #[instrument(skip(client, token))]
    async fn get_user_info<C>(
        &self,
        client: &Client<C>,
        token: &str,
    ) -> Result<UserInfo, UserInfoError>
    where
        C: Connect + Clone + Send + Sync + 'static,
    {
        tracing::trace!("Requesting user information from OAuth provider");

        let request = Request::builder()
            .method(Method::GET)
            .header(header::AUTHORIZATION, format!("Bearer {}", token))
            .uri(self.user_info_endpoint())
            .body(Body::empty())?;

        let response = client.request(request).await?;

        tracing::trace!(status = ?response.status(), "Received response from OAuth provider");

        let body = response.into_body();
        let bytes = to_bytes(body).await?;

        self.extract_user_info(&bytes)
    }
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct OAuthProviderInfo {
    provider: OAuthProviderName,
    client_id: String,
    auth_url_endpoint: String,
    device_code_endpoint: String,
    token_endpoint: String,
    scopes: Vec<String>,
}

#[derive(Debug, Deserialize, PartialEq, Eq, Hash, Serialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum OAuthProviderName {
    Google,
}

impl Display for OAuthProviderName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OAuthProviderName::Google => write!(f, "google"),
        }
    }
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct OAuthProviderNameParam {
    provider: OAuthProviderName,
}
