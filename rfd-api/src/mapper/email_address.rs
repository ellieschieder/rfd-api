use std::collections::BTreeSet;

use async_trait::async_trait;
use rfd_model::storage::StoreError;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{context::ApiContext, endpoints::login::UserInfo, ApiPermissions};

use super::MapperRule;

#[derive(Debug, Deserialize, Serialize)]
pub struct EmailDomainMapper {
    email: String,
    permissions: ApiPermissions,
    groups: Vec<String>,
}

#[async_trait]
impl MapperRule for EmailDomainMapper {
    async fn permissions_for(
        &self,
        _ctx: &ApiContext,
        user: &UserInfo,
    ) -> Result<ApiPermissions, StoreError> {
        if user
            .verified_emails
            .iter()
            .fold(false, |found, email| found || email == &self.email)
        {
            Ok(self.permissions.clone())
        } else {
            Ok(ApiPermissions::new())
        }
    }

    async fn groups_for(
        &self,
        ctx: &ApiContext,
        user: &UserInfo,
    ) -> Result<BTreeSet<Uuid>, StoreError> {
        let found_email = user
            .verified_emails
            .iter()
            .fold(false, |found, email| found || email == &self.email);

        if found_email {
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
                .collect();
            Ok(groups)
        } else {
            Ok(BTreeSet::new())
        }
    }
}
