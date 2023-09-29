use async_trait::async_trait;
use rfd_model::storage::StoreError;
use serde::Deserialize;
use uuid::Uuid;

use crate::{context::ApiContext, endpoints::login::UserInfo, ApiPermissions};

#[async_trait]
pub trait MapperRule: Send + Sync {
    async fn permissions_for(
        &self,
        ctx: &ApiContext,
        user: &UserInfo,
    ) -> Result<ApiPermissions, StoreError>;
    async fn groups_for(&self, ctx: &ApiContext, user: &UserInfo) -> Result<Vec<Uuid>, StoreError>;
}

#[derive(Debug, Deserialize)]
pub enum MapperRules {
    EmailDomain(EmailDomainMapper),
}

#[derive(Debug, Deserialize)]
pub struct EmailDomainMapper {
    domain: String,
    groups: Vec<String>,
}

#[async_trait]
impl MapperRule for EmailDomainMapper {
    async fn permissions_for(
        &self,
        _ctx: &ApiContext,
        _user: &UserInfo,
    ) -> Result<ApiPermissions, StoreError> {
        Ok(ApiPermissions::new())
    }

    async fn groups_for(&self, ctx: &ApiContext, user: &UserInfo) -> Result<Vec<Uuid>, StoreError> {
        let has_email_in_domain = user
            .verified_emails
            .iter()
            .fold(false, |found, email| found || email.ends_with(&self.domain));

        if has_email_in_domain {
            let groups = ctx
                .get_groups()
                .await?
                .into_iter()
                .filter_map(|group| {
                    if self.groups.contains(&group.name) {
                        Some(group.id)
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>();
            Ok(groups)
        } else {
            Ok(vec![])
        }
    }
}

#[async_trait]
impl MapperRule for MapperRules {
    async fn permissions_for(
        &self,
        ctx: &ApiContext,
        user: &UserInfo,
    ) -> Result<ApiPermissions, StoreError> {
        match self {
            Self::EmailDomain(rule) => rule.permissions_for(ctx, user).await,
        }
    }

    async fn groups_for(&self, ctx: &ApiContext, user: &UserInfo) -> Result<Vec<Uuid>, StoreError> {
        match self {
            Self::EmailDomain(rule) => rule.groups_for(ctx, user).await,
        }
    }
}
