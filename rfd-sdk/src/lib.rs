mod generated;

use std::fmt::Display;

pub use generated::sdk::*;
use generated::sdk::types::ApiPermission;
pub use progenitor_client::Error as ProgenitorClientError;

impl Display for ApiPermission {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::CreateApiUserToken(id) => write!(f, "create-token:{}", id),
            Self::CreateApiUserTokenSelf => write!(f, "create-token-self"),
            Self::CreateApiUserTokenAssigned => write!(f, "create-token-assigned"),
            Self::CreateApiUserTokenAll => write!(f, "create-token-all"),
            Self::GetApiUser(id) => write!(f, "get-user:{}", id),
            Self::GetApiUserSelf => write!(f, "get-user-self"),
            Self::GetApiUserAssigned => write!(f, "get-user-assigned"),
            Self::GetApiUserAll => write!(f, "get-user-all"),
            Self::GetApiUserToken(id) => write!(f, "get-token:{}", id),
            Self::GetApiUserTokenSelf => write!(f, "get-token-self"),
            Self::GetApiUserTokenAssigned => write!(f, "get-token-assigned"),
            Self::GetApiUserTokenAll => write!(f, "get-token-all"),
            Self::DeleteApiUserToken(id) => write!(f, "delete-token:{}", id),
            Self::DeleteApiUserTokenSelf => write!(f, "delete-token-self"),
            Self::DeleteApiUserTokenAssigned => write!(f, "delete-token-assigned"),
            Self::DeleteApiUserTokenAll => write!(f, "delete-token-all"),
            Self::CreateApiUser => write!(f, "create-user"),
            Self::UpdateApiUser(id) => write!(f, "update-user:{}", id),
            Self::UpdateApiUserSelf => write!(f, "update-user-self"),
            Self::UpdateApiUserAssigned => write!(f, "update-user-assigned"),
            Self::UpdateApiUserAll => write!(f, "update-user-all"),
            Self::ListGroups => write!(f, "list-groups"),
            Self::CreateGroup => write!(f, "create-group"),
            Self::UpdateGroup(id) => write!(f, "update-group:{}", id),
            Self::AddToGroup(id) => write!(f, "add-group-membership:{}", id),
            Self::RemoveFromGroup(id) => write!(f, "remove-group-membership:{}", id),
            Self::DeleteGroup(id) => write!(f, "delete-group:{}", id),
            Self::GetRfd(number) => write!(f, "get-rfd:{}", number),
            Self::GetRfds(numbers) => write!(f, "get-rfds:{}", numbers.iter().map(|i| i.to_string()).collect::<Vec<_>>().join(",")),
            Self::GetRfdsAssigned => write!(f, "get-rfds-assigned"),
            Self::GetRfdsAll => write!(f, "get-rfds-all"),
            Self::GetDiscussion(number) => write!(f, "get-discussion:{}", number),
            Self::GetDiscussions(numbers) => write!(f, "get-discussions:{}", numbers.iter().map(|i| i.to_string()).collect::<Vec<_>>().join(",")),
            Self::GetDiscussionsAssigned => write!(f, "get-discussions-assigned"),
            Self::GetDiscussionsAll => write!(f, "get-discussions-all"),
            Self::SearchRfds => write!(f, "search-rfds"),
            Self::CreateOAuthClient => write!(f, "create-oauth-client"),
            Self::GetOAuthClient(id) => write!(f, "get-oauth-client:{}", id),
            Self::GetOAuthClients(ids) => write!(f, "get-oauth-clients:{}", ids.iter().map(|i| i.to_string()).collect::<Vec<_>>().join(",")),
            Self::GetOAuthClientsAssigned => write!(f, "get-oauth-clients-assigned"),
            Self::GetOAuthClientsAll => write!(f, "get-oauth-clients-all"),
            Self::UpdateOAuthClient(id) => write!(f, "update-oauth-client:{}", id),
            Self::UpdateOAuthClients(ids) => write!(f, "update-oauth-clients:{}", ids.iter().map(|i| i.to_string()).collect::<Vec<_>>().join(",")),
            Self::UpdateOAuthClientsAssigned => write!(f, "update-oauth-clients-assigned"),
            Self::UpdateOAuthClientsAll => write!(f, "update-oauth-clients-all"),
            Self::DeleteOAuthClient(id) => write!(f, "delete-oauth-client:{}", id),
            Self::DeleteOAuthClients(ids) => write!(f, "delete-oauth-clients:{}", ids.iter().map(|i| i.to_string()).collect::<Vec<_>>().join(",")),
            Self::DeleteOAuthClientsAssigned => write!(f, "delete-oauth-clients-assigned"),
            Self::DeleteOAuthClientsAll => write!(f, "delete-oauth-clients-self"),
        }
    }
}