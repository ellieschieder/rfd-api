use chrono::{DateTime, Duration, Utc};
use dropshot::{HttpError, RequestContext};
use http::StatusCode;
use hyper::{client::HttpConnector, Body, Client};
use hyper_tls::HttpsConnector;
use jsonwebtoken::jwk::JwkSet;
use oauth2::CsrfToken;
use rfd_model::{
    permissions::{Caller, Permissions},
    schema_ext::LoginAttemptState,
    storage::{
        AccessTokenStore, ApiKeyFilter, ApiKeyStore, ApiUserFilter, ApiUserProviderFilter,
        ApiUserProviderStore, ApiUserStore, JobStore, ListPagination, LoginAttemptFilter,
        LoginAttemptStore, RfdFilter, RfdPdfFilter, RfdPdfStore, RfdRevisionFilter,
        RfdRevisionStore, RfdStore, StoreError,
    },
    AccessToken, ApiUser, ApiUserProvider, InvalidValueError, Job, LoginAttempt, NewAccessToken,
    NewApiKey, NewApiUser, NewApiUserProvider, NewJob, NewLoginAttempt,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, sync::Arc};
use tap::TapFallible;
use thiserror::Error;
use tracing::{info_span, instrument, Instrument};
use uuid::Uuid;

use crate::{
    authn::{
        jwt::{key_to_signer, Claims, JwtSigner, SignerError},
        key::{key_to_encryptor, EncryptorError, KeyEncryptor},
        AuthError, AuthToken,
    },
    config::{JwtConfig, PermissionsConfig},
    email_validator::EmailValidator,
    endpoints::login::{
        oauth::{OAuthProvider, OAuthProviderError, OAuthProviderFn, OAuthProviderName},
        LoginError, UserInfo,
    },
    error::{ApiError, AppError},
    permissions::{ApiPermission, ExpandPermission},
    util::response::{client_error, internal_error},
    ApiCaller, ApiPermissions, User, UserToken,
};

pub trait Storage:
    RfdStore
    + RfdRevisionStore
    + RfdPdfStore
    + JobStore
    + ApiUserStore<ApiPermission>
    + ApiKeyStore<ApiPermission>
    + ApiUserProviderStore
    + AccessTokenStore
    + LoginAttemptStore
    + Send
    + Sync
    + 'static
{
}
impl<T> Storage for T where
    T: RfdStore
        + RfdRevisionStore
        + RfdPdfStore
        + JobStore
        + ApiUserStore<ApiPermission>
        + ApiKeyStore<ApiPermission>
        + ApiUserProviderStore
        + AccessTokenStore
        + LoginAttemptStore
        + Send
        + Sync
        + 'static
{
}

pub struct ApiContext {
    pub email_validator: Arc<dyn EmailValidator + Send + Sync>,
    pub https_client: Client<HttpsConnector<HttpConnector>, Body>,
    pub public_url: String,
    pub storage: Arc<dyn Storage>,
    pub permissions: PermissionsContext,
    pub jwt: JwtContext,
    pub api_key: ApiKeyContext,
    pub oauth_providers: HashMap<OAuthProviderName, Box<dyn OAuthProviderFn>>,
}

pub struct PermissionsContext {
    pub default: ApiPermissions,
}

pub struct JwtContext {
    pub default_expiration: i64,
    pub max_expiration: i64,
    pub keys: Vec<Box<dyn JwtSigner<Claims = Claims>>>,
    pub jwks: JwkSet,
}

pub struct ApiKeyContext {
    pub encryptor: Box<dyn KeyEncryptor>,
}

pub struct RegisteredAccessToken {
    pub access_token: AccessToken,
    pub signed_token: String,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Error)]
pub enum CallerError {
    #[error("Failed to authenticate caller")]
    FailedToAuthenticate,
    #[error("Inner storage failure: {0}")]
    Storage(#[from] StoreError),
}

impl From<CallerError> for HttpError {
    fn from(error: CallerError) -> Self {
        tracing::info!(?error, "Failed to authenticate caller");

        match error {
            CallerError::FailedToAuthenticate => {
                client_error(StatusCode::UNAUTHORIZED, "Failed to authenticate")
            }
            CallerError::Storage(_) => internal_error("Internal storage failed"),
        }
    }
}

#[derive(Debug, Error)]
pub enum LoginAttemptError {
    #[error(transparent)]
    FailedToCreate(#[from] InvalidValueError),
    #[error(transparent)]
    Storage(#[from] StoreError),
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct FullRfd {
    pub id: Uuid,
    pub rfd_number: i32,
    pub link: Option<String>,
    pub discussion: Option<String>,
    pub title: String,
    pub state: Option<String>,
    pub authors: Option<String>,
    pub content: String,
    pub sha: String,
    pub commit: String,
    pub committed_at: DateTime<Utc>,
    pub pdfs: Vec<FullRfdPdfEntry>,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct FullRfdPdfEntry {
    pub source: String,
    pub link: String,
}

impl ApiContext {
    pub async fn new(
        email_validator: Arc<dyn EmailValidator + Send + Sync>,
        public_url: String,
        storage: Arc<dyn Storage>,
        permissions: PermissionsConfig,
        jwt: JwtConfig,
    ) -> Result<Self, AppError> {
        let mut keys = vec![];

        for key in &jwt.keys {
            keys.push(key_to_signer(key).await?);
        }

        Ok(Self {
            email_validator,
            https_client: hyper::Client::builder().build(HttpsConnector::new()),
            public_url,
            storage,
            permissions: PermissionsContext {
                default: permissions.default.into(),
            },
            jwt: JwtContext {
                default_expiration: jwt.default_expiration,
                max_expiration: jwt.max_expiration,
                jwks: JwkSet {
                    keys: keys.iter().map(|k| k.jwk()).cloned().collect(),
                },
                keys,
            },
            api_key: ApiKeyContext {
                encryptor: key_to_encryptor(&jwt.keys[0]).await?,
            },
            oauth_providers: HashMap::new(),
        })
    }

    pub async fn authn_token(&self, rqctx: &RequestContext<Self>) -> Result<AuthToken, AuthError> {
        AuthToken::extract(rqctx).await
    }

    pub fn default_jwt_expiration(&self) -> i64 {
        self.jwt.default_expiration
    }

    pub fn max_jwt_expiration(&self) -> i64 {
        self.jwt.max_expiration
    }

    pub async fn jwks(&self) -> &JwkSet {
        &self.jwt.jwks
    }

    pub async fn sign(&self, claims: &Claims) -> Result<String, SignerError> {
        let signer = self.jwt.keys.first().unwrap();
        signer.sign(claims).await
    }

    pub async fn encrypt(&self, value: &str) -> Result<String, EncryptorError> {
        self.api_key.encryptor.encrypt(value).await
    }

    #[instrument(skip(self, auth))]
    pub async fn get_caller(&self, auth: &AuthToken) -> Result<ApiCaller, CallerError> {
        let (api_user_id, permissions) = match auth {
            AuthToken::ApiKey(api_key) => {
                async {
                    tracing::debug!("Attempt to authenticate");

                    let mut key = ApiKeyStore::list(
                        &*self.storage,
                        ApiKeyFilter {
                            key: Some(vec![api_key.encrypted.to_string()]),
                            expired: false,
                            deleted: false,
                            ..Default::default()
                        },
                        &ListPagination {
                            offset: 0,
                            limit: 1,
                        },
                    )
                    .await?;

                    if let Some(key) = key.pop() {
                        tracing::debug!("Verified caller key");

                        Ok((key.api_user_id, key.permissions))
                    } else {
                        tracing::debug!("Failed to find matching key");
                        Err(CallerError::FailedToAuthenticate)
                    }
                }
                .instrument(info_span!("Test api key"))
                .await
            }
            AuthToken::Jwt(jwt) => {
                // AuthnToken::Jwt can only be generated from a verified JWT
                Ok((jwt.claims.aud, jwt.claims.prm.clone()))
            }
        }?;

        // The permissions for the caller is the intersection of the user's permissions and the tokens permissions
        if let Some(user) = ApiUserStore::get(&*self.storage, &api_user_id, false).await? {
            let caller = Caller {
                id: api_user_id,
                permissions: user.permissions.intersect(&permissions).expand(&user),
            };

            tracing::info!(?caller, "Resolved caller");

            Ok(caller)
        } else {
            tracing::error!("User for verified token does not exist");
            Err(CallerError::FailedToAuthenticate)
        }
    }

    pub async fn is_empty(&self) -> Result<bool, StoreError> {
        let mut user_filter = ApiUserFilter::default();
        user_filter.deleted = true;

        let users =
            ApiUserStore::list(&*self.storage, user_filter, &ListPagination::latest()).await?;

        let mut token_filter = ApiKeyFilter::default();
        token_filter.deleted = true;

        let tokens =
            ApiKeyStore::list(&*self.storage, token_filter, &ListPagination::latest()).await?;

        Ok(users.len() == 0 && tokens.len() == 0)
    }

    pub fn insert_oauth_provider(
        &mut self,
        name: OAuthProviderName,
        provider_fn: Box<dyn OAuthProviderFn>,
    ) {
        self.oauth_providers.insert(name, provider_fn);
    }

    pub async fn get_oauth_provider(
        &self,
        provider: &OAuthProviderName,
    ) -> Result<Box<dyn OAuthProvider + Send + Sync>, OAuthProviderError> {
        self.oauth_providers
            .get(provider)
            .map(|factory| (*factory)())
            .ok_or(OAuthProviderError::FailToCreateInvalidProvider)
    }

    // RFD Operations

    pub async fn get_rfd(
        &self,
        rfd_number: i32,
        sha: Option<String>,
    ) -> Result<Option<FullRfd>, StoreError> {
        let rfds = RfdStore::list(
            &*self.storage,
            RfdFilter::default().rfd_number(Some(vec![rfd_number])),
            &ListPagination::default().limit(1),
        )
        .await?;

        if let Some(rfd) = rfds.into_iter().nth(0) {
            let latest_revision = RfdRevisionStore::list(
                &*self.storage,
                RfdRevisionFilter::default()
                    .rfd(Some(vec![rfd.id]))
                    .sha(sha.map(|sha| vec![sha])),
                &ListPagination::default().limit(1),
            )
            .await?;

            if let Some(revision) = latest_revision.into_iter().nth(0) {
                let pdfs = RfdPdfStore::list(
                    &*self.storage,
                    RfdPdfFilter::default().rfd_revision(Some(vec![revision.id])),
                    &ListPagination::default(),
                )
                .await?;

                Ok(Some(FullRfd {
                    id: rfd.id,
                    rfd_number: rfd.rfd_number,
                    link: rfd.link,
                    discussion: revision.discussion,
                    title: revision.title,
                    state: revision.state,
                    authors: revision.authors,
                    content: revision.content,
                    sha: revision.sha,
                    commit: revision.commit_sha,
                    committed_at: revision.committed_at,
                    pdfs: pdfs
                        .into_iter()
                        .map(|pdf| FullRfdPdfEntry {
                            source: pdf.source.to_string(),
                            link: pdf.link,
                        })
                        .collect(),
                }))
            } else {
                Ok(None)
            }
        } else {
            Ok(None)
        }
    }

    // Webhook Operations

    pub async fn register_job(&self, new_job: NewJob) -> Result<Job, StoreError> {
        JobStore::upsert(&*self.storage, new_job).await
    }

    // Login Operations

    #[instrument(skip(self, info), fields(info.external_id))]
    pub async fn register_api_user(&self, info: UserInfo) -> Result<User, ApiError> {
        // Check if we have seen this identity before
        let mut filter = ApiUserProviderFilter::default();
        filter.provider = Some(vec![info.external_id.provider().to_string()]);
        filter.provider_id = Some(vec![info.external_id.id().to_string()]);

        tracing::info!("Check for existing users matching the requested external id");

        let api_user_providers = self
            .list_api_user_provider(filter, &ListPagination::latest())
            .await?;

        match api_user_providers.len() {
            0 => {
                tracing::info!("Did not find any existing users. Registering a new user.");

                let user = self.ensure_api_user(Uuid::new_v4()).await?;
                self.update_api_user_provider(NewApiUserProvider {
                    id: Uuid::new_v4(),
                    api_user_id: user.id,
                    emails: info.verified_emails,
                    provider: info.external_id.provider().to_string(),
                    provider_id: info.external_id.id().to_string(),
                })
                .await?;

                Ok(user)
            }
            1 => {
                tracing::info!("Found an existing user. Attaching provider.");

                // This branch ensures that there is a 0th indexed item
                let provider = api_user_providers.into_iter().nth(0).unwrap();
                Ok(self.ensure_api_user(provider.api_user_id).await?)
            }
            _ => {
                // If we found more than one provider, then we have encountered an inconsistency in
                // our database.
                tracing::error!(
                    count = api_user_providers.len(),
                    "Found multiple providers for external id"
                );

                Err(StoreError::InvariantFailed(
                    "Multiple providers for external id found".to_string(),
                )
                .into())
            }
        }
    }

    async fn ensure_api_user(&self, api_user_id: Uuid) -> Result<User, ApiError> {
        match self.get_api_user(&api_user_id).await? {
            Some(api_user) => Ok(api_user),
            None => self
                .update_api_user(NewApiUser {
                    id: api_user_id,
                    permissions: self.permissions.default.clone(),
                })
                .await
                .map_err(ApiError::Storage)
                .tap_err(|err| {
                    tracing::error!(
                        ?err,
                        "Failed to create new api user for OAuth authenticated user"
                    )
                }),
        }
    }

    pub async fn register_access_token(
        &self,
        api_user: &ApiUser<ApiPermission>,
        requested_permissions: &Permissions<ApiPermission>,
        expires_at: Option<DateTime<Utc>>,
    ) -> Result<RegisteredAccessToken, ApiError> {
        let expires_at = expires_at
            .unwrap_or_else(|| Utc::now() + Duration::seconds(self.default_jwt_expiration()));

        if expires_at > Utc::now() + Duration::seconds(self.max_jwt_expiration()) {
            return Err(ApiError::Login(LoginError::ExcessTokenExpiration));
        }

        // Take the intersection of the api user permissions and the requested permissions. Tokens
        // should never have permissions that are wider than the user's permissions
        let permissions = requested_permissions.intersect(&api_user.permissions);

        // Ensure that the token is within the configured limits
        let claims = Claims {
            aud: api_user.id,
            prm: permissions,
            exp: expires_at.timestamp(),
            nbf: Utc::now().timestamp(),
            jti: Uuid::new_v4(),
        };

        let token = self
            .create_access_token(NewAccessToken {
                id: claims.jti,
                api_user_id: api_user.id,
                revoked_at: None,
            })
            .await?;

        let signed = self.sign(&claims).await?;

        Ok(RegisteredAccessToken {
            access_token: token,
            signed_token: signed,
            expires_at,
        })
    }

    // API User Operations

    pub async fn get_api_user(&self, id: &Uuid) -> Result<Option<User>, StoreError> {
        ApiUserStore::get(&*self.storage, id, false).await
    }

    pub async fn list_api_user(
        &self,
        filter: ApiUserFilter,
        pagination: &ListPagination,
    ) -> Result<Vec<User>, StoreError> {
        ApiUserStore::list(&*self.storage, filter, pagination).await
    }

    pub async fn update_api_user(
        &self,
        api_user: NewApiUser<ApiPermission>,
    ) -> Result<User, StoreError> {
        ApiUserStore::upsert(&*self.storage, api_user).await
    }

    pub async fn create_api_user_token(
        &self,
        token: NewApiKey<ApiPermission>,
        api_user: &ApiUser<ApiPermission>,
    ) -> Result<UserToken, StoreError> {
        ApiKeyStore::upsert(&*self.storage, token, api_user).await
    }

    pub async fn get_api_user_token(&self, id: &Uuid) -> Result<Option<UserToken>, StoreError> {
        ApiKeyStore::get(&*self.storage, id, false).await
    }

    pub async fn get_api_user_tokens(
        &self,
        api_user_id: &Uuid,
        pagination: &ListPagination,
    ) -> Result<Vec<UserToken>, StoreError> {
        ApiKeyStore::list(
            &*self.storage,
            ApiKeyFilter {
                api_user_id: Some(vec![*api_user_id]),
                expired: true,
                deleted: false,
                ..Default::default()
            },
            pagination,
        )
        .await
    }

    pub async fn get_api_user_provider(
        &self,
        id: &Uuid,
    ) -> Result<Option<ApiUserProvider>, StoreError> {
        ApiUserProviderStore::get(&*self.storage, id, false).await
    }

    pub async fn list_api_user_provider(
        &self,
        filter: ApiUserProviderFilter,
        pagination: &ListPagination,
    ) -> Result<Vec<ApiUserProvider>, StoreError> {
        ApiUserProviderStore::list(&*self.storage, filter, pagination).await
    }

    pub async fn update_api_user_provider(
        &self,
        api_user: NewApiUserProvider,
    ) -> Result<ApiUserProvider, StoreError> {
        ApiUserProviderStore::upsert(&*self.storage, api_user).await
    }

    pub async fn delete_api_user_token(&self, id: &Uuid) -> Result<Option<UserToken>, StoreError> {
        ApiKeyStore::delete(&*self.storage, id).await
    }

    pub async fn create_access_token(
        &self,
        access_token: NewAccessToken,
    ) -> Result<AccessToken, StoreError> {
        AccessTokenStore::upsert(&*self.storage, access_token).await
    }

    pub async fn create_login_attempt(
        &self,
        attempt: NewLoginAttempt,
    ) -> Result<LoginAttempt, StoreError> {
        LoginAttemptStore::upsert(&*self.storage, attempt).await
    }

    pub async fn set_login_provider_authz_code(
        &self,
        attempt: LoginAttempt,
        code: String,
    ) -> Result<LoginAttempt, StoreError> {
        let mut attempt: NewLoginAttempt = attempt.into();
        attempt.provider_authz_code = Some(code);

        // TODO: Internal state changes to the struct
        attempt.attempt_state = LoginAttemptState::RemoteAuthenticated;
        attempt.authz_code = Some(CsrfToken::new_random().secret().to_string());

        LoginAttemptStore::upsert(&*self.storage, attempt).await
    }

    pub async fn get_login_attempt(&self, id: &Uuid) -> Result<Option<LoginAttempt>, StoreError> {
        LoginAttemptStore::get(&*self.storage, id).await
    }

    pub async fn get_login_attempt_for_code(
        &self,
        code: &str,
    ) -> Result<Option<LoginAttempt>, StoreError> {
        let filter = LoginAttemptFilter {
            attempt_state: Some(vec![LoginAttemptState::RemoteAuthenticated]),
            authz_code: Some(vec![code.to_string()]),
            ..Default::default()
        };

        let mut attempts = LoginAttemptStore::list(
            &*self.storage,
            filter,
            &ListPagination {
                offset: 0,
                limit: 1,
            },
        )
        .await?;

        Ok(attempts.pop())
    }

    pub async fn fail_login_attempt(
        &self,
        attempt: LoginAttempt,
    ) -> Result<LoginAttempt, StoreError> {
        let mut attempt: NewLoginAttempt = attempt.into();
        attempt.attempt_state = LoginAttemptState::Failed;
        LoginAttemptStore::upsert(&*self.storage, attempt).await
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use async_trait::async_trait;
    use rfd_model::{
        permissions::Caller,
        storage::{
            AccessTokenStore, ApiKeyStore, ApiUserProviderStore, ApiUserStore, JobStore,
            ListPagination, LoginAttemptStore, MockAccessTokenStore, MockApiKeyStore,
            MockApiUserProviderStore, MockApiUserStore, MockJobStore, MockLoginAttemptStore,
            MockRfdPdfStore, MockRfdRevisionStore, MockRfdStore, RfdPdfStore, RfdRevisionStore,
            RfdStore,
        },
        ApiKey, ApiUserProvider, NewAccessToken, NewApiKey, NewApiUser, NewApiUserProvider, NewJob,
        NewLoginAttempt, NewRfd, NewRfdPdf, NewRfdRevision,
    };
    use std::sync::Arc;

    use crate::permissions::ApiPermission;

    // Construct a mock storage engine that can be wrapped in an ApiContext for testing
    pub struct MockStorage {
        pub caller: Option<Caller<ApiPermission>>,
        pub rfd_store: Option<Arc<MockRfdStore>>,
        pub rfd_revision_store: Option<Arc<MockRfdRevisionStore>>,
        pub rfd_pdf_store: Option<Arc<MockRfdPdfStore>>,
        pub job_store: Option<Arc<MockJobStore>>,
        pub api_user_store: Option<Arc<MockApiUserStore<ApiPermission>>>,
        pub api_user_token_store: Option<Arc<MockApiKeyStore<ApiPermission>>>,
        pub api_user_provider_store: Option<Arc<MockApiUserProviderStore>>,
        pub device_token_store: Option<Arc<MockAccessTokenStore>>,
        pub login_attempt_store: Option<Arc<MockLoginAttemptStore>>,
    }

    impl MockStorage {
        pub fn new() -> Self {
            Self {
                caller: None,
                rfd_store: None,
                rfd_revision_store: None,
                rfd_pdf_store: None,
                job_store: None,
                api_user_store: None,
                api_user_token_store: None,
                api_user_provider_store: None,
                device_token_store: None,
                login_attempt_store: None,
            }
        }
    }

    #[async_trait]
    impl RfdStore for MockStorage {
        async fn get(
            &self,
            id: &uuid::Uuid,
            deleted: bool,
        ) -> Result<Option<rfd_model::Rfd>, rfd_model::storage::StoreError> {
            self.rfd_store.as_ref().unwrap().get(id, deleted).await
        }

        async fn list(
            &self,
            filter: rfd_model::storage::RfdFilter,
            pagination: &ListPagination,
        ) -> Result<Vec<rfd_model::Rfd>, rfd_model::storage::StoreError> {
            self.rfd_store
                .as_ref()
                .unwrap()
                .list(filter, pagination)
                .await
        }

        async fn upsert(
            &self,
            new_rfd: NewRfd,
        ) -> Result<rfd_model::Rfd, rfd_model::storage::StoreError> {
            self.rfd_store.as_ref().unwrap().upsert(new_rfd).await
        }

        async fn delete(
            &self,
            id: &uuid::Uuid,
        ) -> Result<Option<rfd_model::Rfd>, rfd_model::storage::StoreError> {
            self.rfd_store.as_ref().unwrap().delete(id).await
        }
    }

    #[async_trait]
    impl RfdRevisionStore for MockStorage {
        async fn get(
            &self,
            id: &uuid::Uuid,
            deleted: bool,
        ) -> Result<Option<rfd_model::RfdRevision>, rfd_model::storage::StoreError> {
            self.rfd_revision_store
                .as_ref()
                .unwrap()
                .get(id, deleted)
                .await
        }

        async fn list(
            &self,
            filter: rfd_model::storage::RfdRevisionFilter,
            pagination: &ListPagination,
        ) -> Result<Vec<rfd_model::RfdRevision>, rfd_model::storage::StoreError> {
            self.rfd_revision_store
                .as_ref()
                .unwrap()
                .list(filter, pagination)
                .await
        }

        async fn upsert(
            &self,
            new_revision: NewRfdRevision,
        ) -> Result<rfd_model::RfdRevision, rfd_model::storage::StoreError> {
            self.rfd_revision_store
                .as_ref()
                .unwrap()
                .upsert(new_revision)
                .await
        }

        async fn delete(
            &self,
            id: &uuid::Uuid,
        ) -> Result<Option<rfd_model::RfdRevision>, rfd_model::storage::StoreError> {
            self.rfd_revision_store.as_ref().unwrap().delete(id).await
        }
    }

    #[async_trait]
    impl RfdPdfStore for MockStorage {
        async fn get(
            &self,
            id: &uuid::Uuid,
            deleted: bool,
        ) -> Result<Option<rfd_model::RfdPdf>, rfd_model::storage::StoreError> {
            self.rfd_pdf_store.as_ref().unwrap().get(id, deleted).await
        }

        async fn list(
            &self,
            filter: rfd_model::storage::RfdPdfFilter,
            pagination: &ListPagination,
        ) -> Result<Vec<rfd_model::RfdPdf>, rfd_model::storage::StoreError> {
            self.rfd_pdf_store
                .as_ref()
                .unwrap()
                .list(filter, pagination)
                .await
        }

        async fn upsert(
            &self,
            new_pdf: NewRfdPdf,
        ) -> Result<rfd_model::RfdPdf, rfd_model::storage::StoreError> {
            self.rfd_pdf_store.as_ref().unwrap().upsert(new_pdf).await
        }

        async fn delete(
            &self,
            id: &uuid::Uuid,
        ) -> Result<Option<rfd_model::RfdPdf>, rfd_model::storage::StoreError> {
            self.rfd_pdf_store.as_ref().unwrap().delete(id).await
        }
    }

    #[async_trait]
    impl JobStore for MockStorage {
        async fn get(
            &self,
            id: i32,
        ) -> Result<Option<rfd_model::Job>, rfd_model::storage::StoreError> {
            self.job_store.as_ref().unwrap().get(id).await
        }

        async fn list(
            &self,
            filter: rfd_model::storage::JobFilter,
            pagination: &ListPagination,
        ) -> Result<Vec<rfd_model::Job>, rfd_model::storage::StoreError> {
            self.job_store
                .as_ref()
                .unwrap()
                .list(filter, pagination)
                .await
        }

        async fn upsert(
            &self,
            new_job: NewJob,
        ) -> Result<rfd_model::Job, rfd_model::storage::StoreError> {
            self.job_store.as_ref().unwrap().upsert(new_job).await
        }

        async fn complete(
            &self,
            id: i32,
        ) -> Result<Option<rfd_model::Job>, rfd_model::storage::StoreError> {
            self.job_store.as_ref().unwrap().complete(id).await
        }
    }

    #[async_trait]
    impl ApiUserStore<ApiPermission> for MockStorage {
        async fn get(
            &self,
            id: &uuid::Uuid,
            deleted: bool,
        ) -> Result<Option<rfd_model::ApiUser<ApiPermission>>, rfd_model::storage::StoreError>
        {
            self.api_user_store.as_ref().unwrap().get(id, deleted).await
        }

        async fn list(
            &self,
            filter: rfd_model::storage::ApiUserFilter,
            pagination: &ListPagination,
        ) -> Result<Vec<rfd_model::ApiUser<ApiPermission>>, rfd_model::storage::StoreError>
        {
            self.api_user_store
                .as_ref()
                .unwrap()
                .list(filter, pagination)
                .await
        }

        async fn upsert(
            &self,
            api_user: NewApiUser<ApiPermission>,
        ) -> Result<rfd_model::ApiUser<ApiPermission>, rfd_model::storage::StoreError> {
            self.api_user_store.as_ref().unwrap().upsert(api_user).await
        }

        async fn delete(
            &self,
            id: &uuid::Uuid,
        ) -> Result<Option<rfd_model::ApiUser<ApiPermission>>, rfd_model::storage::StoreError>
        {
            self.api_user_store.as_ref().unwrap().delete(id).await
        }
    }

    #[async_trait]
    impl ApiKeyStore<ApiPermission> for MockStorage {
        async fn get(
            &self,
            id: &uuid::Uuid,
            deleted: bool,
        ) -> Result<Option<ApiKey<ApiPermission>>, rfd_model::storage::StoreError> {
            self.api_user_token_store
                .as_ref()
                .unwrap()
                .get(id, deleted)
                .await
        }

        async fn list(
            &self,
            filter: rfd_model::storage::ApiKeyFilter,
            pagination: &ListPagination,
        ) -> Result<Vec<ApiKey<ApiPermission>>, rfd_model::storage::StoreError> {
            self.api_user_token_store
                .as_ref()
                .unwrap()
                .list(filter, pagination)
                .await
        }

        async fn upsert(
            &self,
            token: NewApiKey<ApiPermission>,
            api_user: &rfd_model::ApiUser<ApiPermission>,
        ) -> Result<ApiKey<ApiPermission>, rfd_model::storage::StoreError> {
            self.api_user_token_store
                .as_ref()
                .unwrap()
                .upsert(token, api_user)
                .await
        }

        async fn delete(
            &self,
            id: &uuid::Uuid,
        ) -> Result<Option<ApiKey<ApiPermission>>, rfd_model::storage::StoreError> {
            self.api_user_token_store.as_ref().unwrap().delete(id).await
        }
    }

    #[async_trait]
    impl ApiUserProviderStore for MockStorage {
        async fn get(
            &self,
            id: &uuid::Uuid,
            deleted: bool,
        ) -> Result<Option<ApiUserProvider>, rfd_model::storage::StoreError> {
            self.api_user_provider_store
                .as_ref()
                .unwrap()
                .get(id, deleted)
                .await
        }

        async fn list(
            &self,
            filter: rfd_model::storage::ApiUserProviderFilter,
            pagination: &ListPagination,
        ) -> Result<Vec<ApiUserProvider>, rfd_model::storage::StoreError> {
            self.api_user_provider_store
                .as_ref()
                .unwrap()
                .list(filter, pagination)
                .await
        }

        async fn upsert(
            &self,
            provider: NewApiUserProvider,
        ) -> Result<ApiUserProvider, rfd_model::storage::StoreError> {
            self.api_user_provider_store
                .as_ref()
                .unwrap()
                .upsert(provider)
                .await
        }

        async fn delete(
            &self,
            id: &uuid::Uuid,
        ) -> Result<Option<ApiUserProvider>, rfd_model::storage::StoreError> {
            self.api_user_provider_store
                .as_ref()
                .unwrap()
                .delete(id)
                .await
        }
    }

    #[async_trait]
    impl AccessTokenStore for MockStorage {
        async fn get(
            &self,
            id: &uuid::Uuid,
            revoked: bool,
        ) -> Result<Option<rfd_model::AccessToken>, rfd_model::storage::StoreError> {
            self.device_token_store
                .as_ref()
                .unwrap()
                .get(id, revoked)
                .await
        }

        async fn list(
            &self,
            filter: rfd_model::storage::AccessTokenFilter,
            pagination: &ListPagination,
        ) -> Result<Vec<rfd_model::AccessToken>, rfd_model::storage::StoreError> {
            self.device_token_store
                .as_ref()
                .unwrap()
                .list(filter, pagination)
                .await
        }

        async fn upsert(
            &self,
            token: NewAccessToken,
        ) -> Result<rfd_model::AccessToken, rfd_model::storage::StoreError> {
            self.device_token_store
                .as_ref()
                .unwrap()
                .upsert(token)
                .await
        }
    }

    #[async_trait]
    impl LoginAttemptStore for MockStorage {
        async fn get(
            &self,
            id: &uuid::Uuid,
        ) -> Result<Option<rfd_model::LoginAttempt>, rfd_model::storage::StoreError> {
            self.login_attempt_store.as_ref().unwrap().get(id).await
        }

        async fn list(
            &self,
            filter: rfd_model::storage::LoginAttemptFilter,
            pagination: &ListPagination,
        ) -> Result<Vec<rfd_model::LoginAttempt>, rfd_model::storage::StoreError> {
            self.login_attempt_store
                .as_ref()
                .unwrap()
                .list(filter, pagination)
                .await
        }

        async fn upsert(
            &self,
            attempt: NewLoginAttempt,
        ) -> Result<rfd_model::LoginAttempt, rfd_model::storage::StoreError> {
            self.login_attempt_store
                .as_ref()
                .unwrap()
                .upsert(attempt)
                .await
        }
    }
}
