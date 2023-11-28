// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use dropshot::{endpoint, HttpError, HttpResponseOk, Path, Query, RequestContext};
use http::StatusCode;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use trace_request::trace_request;
use tracing::instrument;

use crate::{
    caller::CallerExt,
    context::{ApiContext, FullRfd, ListRfd},
    error::ApiError,
    permissions::ApiPermission,
    search::SearchRequest,
    util::{
        response::{client_error, internal_error, not_found, unauthorized},
        Timer,
    },
    ApiCaller,
};

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RfdPathParams {
    number: String,
}

/// List all available RFDs
#[trace_request]
#[endpoint {
    method = GET,
    path = "/rfd",
}]
#[instrument(skip(rqctx), fields(request_id = rqctx.request_id), err(Debug))]
pub async fn get_rfds(
    rqctx: RequestContext<ApiContext>,
) -> Result<HttpResponseOk<Vec<ListRfd>>, HttpError> {
    let ctx = rqctx.context();
    let auth = ctx.authn_token(&rqctx).await?;
    get_rfds_op(ctx, &ctx.get_caller(auth.as_ref()).await?).await
}

#[instrument(skip(ctx, caller), fields(caller = ?caller.id), err(Debug))]
async fn get_rfds_op(
    ctx: &ApiContext,
    caller: &ApiCaller,
) -> Result<HttpResponseOk<Vec<ListRfd>>, HttpError> {
    let rfds = ctx
        .list_rfds(caller, None)
        .await
        .map_err(ApiError::Storage)?;
    Ok(HttpResponseOk(rfds))
}

/// Get the latest representation of an RFD
#[trace_request]
#[endpoint {
    method = GET,
    path = "/rfd/{number}",
}]
#[instrument(skip(rqctx), fields(request_id = rqctx.request_id), err(Debug))]
pub async fn get_rfd(
    rqctx: RequestContext<ApiContext>,
    path: Path<RfdPathParams>,
) -> Result<HttpResponseOk<FullRfd>, HttpError> {
    let ctx = rqctx.context();
    let auth = ctx.authn_token(&rqctx).await?;
    get_rfd_op(
        ctx,
        &ctx.get_caller(auth.as_ref()).await?,
        path.into_inner().number,
    )
    .await
}

#[instrument(skip(ctx, caller), fields(caller = ?caller.id), err(Debug))]
async fn get_rfd_op(
    ctx: &ApiContext,
    caller: &ApiCaller,
    number: String,
) -> Result<HttpResponseOk<FullRfd>, HttpError> {
    if let Ok(rfd_number) = number.parse::<i32>() {
        match ctx.get_rfd(caller, rfd_number, None).await {
            Ok(result) => match result {
                Some(rfd) => Ok(HttpResponseOk(rfd)),
                None => {
                    tracing::error!(?rfd_number, "Failed to find RFD");
                    Err(not_found("Failed to find RFD"))
                }
            },
            Err(err) => {
                tracing::error!(?rfd_number, ?err, "Looking up RFD failed");
                Err(internal_error("Failed to lookup RFD"))
            }
        }
    } else {
        Err(client_error(
            StatusCode::BAD_REQUEST,
            "Malformed RFD number",
        ))
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RfdSearchQuery {
    q: String,
    limit: Option<u32>,
    offset: Option<u32>,
    highlight_pre_tag: Option<String>,
    highlight_post_tag: Option<String>,
    attributes_to_crop: Option<String>,
}

/// Search the RFD index and get a list of results
#[trace_request]
#[endpoint {
    method = GET,
    path = "/rfd-search",
}]
#[instrument(skip(rqctx), fields(request_id = rqctx.request_id), err(Debug))]
pub async fn search_rfds(
    rqctx: RequestContext<ApiContext>,
    query: Query<RfdSearchQuery>,
) -> Result<HttpResponseOk<SearchResults>, HttpError> {
    let timer = Timer::new();
    let ctx = rqctx.context();
    let auth = ctx.authn_token(&rqctx).await?;
    let caller = ctx.get_caller(auth.as_ref()).await?;
    tracing::info!(elapsed = ?timer.mark(), "Resolved caller");
    search_rfds_op(ctx, &caller, query.into_inner(), timer).await
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct SearchResults {
    hits: Vec<SearchResultHit>,
    query: String,
    limit: Option<usize>,
    offset: Option<usize>,
}

// TODO: This should be a shared type across the api and processor, but it likely needs custom
// deserialization, serialization, and schema implementations
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct SearchResultHit {
    hierarchy: [Option<String>; 6],
    hierarchy_radio: [Option<String>; 6],
    content: String,
    object_id: String,
    rfd_number: u64,
    anchor: Option<String>,
    url: Option<String>,
    formatted: Option<FormattedSearchResultHit>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct FormattedSearchResultHit {
    hierarchy: [Option<String>; 6],
    hierarchy_radio: [Option<String>; 6],
    content: Option<String>,
    object_id: String,
    rfd_number: u64,
    anchor: Option<String>,
    url: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
struct MeiliSearchResult {
    hierarchy_radio_lvl0: Option<String>,
    hierarchy_radio_lvl1: Option<String>,
    hierarchy_radio_lvl2: Option<String>,
    hierarchy_radio_lvl3: Option<String>,
    hierarchy_radio_lvl4: Option<String>,
    hierarchy_radio_lvl5: Option<String>,
    hierarchy_lvl0: Option<String>,
    hierarchy_lvl1: Option<String>,
    hierarchy_lvl2: Option<String>,
    hierarchy_lvl3: Option<String>,
    hierarchy_lvl4: Option<String>,
    hierarchy_lvl5: Option<String>,
    content: String,
    #[serde(rename = "objectID")]
    object_id: String,
    rfd_number: u64,
    anchor: Option<String>,
    url: Option<String>,
}

#[instrument(skip(ctx, caller, timer), fields(caller = ?caller.id), err(Debug))]
async fn search_rfds_op(
    ctx: &ApiContext,
    caller: &ApiCaller,
    query: RfdSearchQuery,
    timer: Timer,
) -> Result<HttpResponseOk<SearchResults>, HttpError> {
    if caller.can(&ApiPermission::SearchRfds) {
        tracing::debug!(elapsed = ?timer.mark(), "Fetching from remote search API");

        let filter = if caller.can(&ApiPermission::GetRfdsAll) {
            None
        } else {
            let mut filter = "public = true".to_string();

            let allowed_rfds = caller
                .allow_rfds()
                .iter()
                .map(|num| num.to_string())
                .collect::<Vec<_>>()
                .join(", ");
            if allowed_rfds.len() > 0 {
                filter = filter + &format!("OR rfd_number in [{}]", allowed_rfds);
            }

            Some(filter)
        };

        let results = ctx
            .search
            .client
            .search::<MeiliSearchResult>(&SearchRequest {
                q: query.q,
                filter,
                attributes_to_highlight: vec!["*".to_string()],
                highlight_pre_tag: query.highlight_pre_tag,
                highlight_post_tag: query.highlight_post_tag,
                attributes_to_crop: query
                    .attributes_to_crop
                    .unwrap_or_default()
                    .split(",")
                    .map(|s| s.to_string())
                    .collect::<Vec<_>>(),
                limit: query.limit,
                offset: query.offset,
            })
            .await;

        tracing::debug!(elapsed = ?timer.mark(), "Fetched results from remote search");

        match results {
            Ok(results) => {
                let results = SearchResults {
                    hits: results
                        .hits
                        .into_iter()
                        .map(|hit| SearchResultHit {
                            hierarchy_radio: [
                                hit.result.hierarchy_radio_lvl0,
                                hit.result.hierarchy_radio_lvl1,
                                hit.result.hierarchy_radio_lvl2,
                                hit.result.hierarchy_radio_lvl3,
                                hit.result.hierarchy_radio_lvl4,
                                hit.result.hierarchy_radio_lvl5,
                            ],
                            hierarchy: [
                                hit.result.hierarchy_lvl0,
                                hit.result.hierarchy_lvl1,
                                hit.result.hierarchy_lvl2,
                                hit.result.hierarchy_lvl3,
                                hit.result.hierarchy_lvl4,
                                hit.result.hierarchy_lvl5,
                            ],
                            content: hit.result.content,
                            object_id: hit.result.object_id.clone(),
                            rfd_number: hit.result.rfd_number.clone(),
                            anchor: hit.result.anchor,
                            url: hit.result.url,
                            formatted: hit.formatted_result.map(|formatted| {
                                FormattedSearchResultHit {
                                    hierarchy_radio: [
                                        formatted
                                            .get("hierarchy_radio_lvl0")
                                            .and_then(|v| v.as_str().map(|s| s.to_string())),
                                        formatted
                                            .get("hierarchy_radio_lvl1")
                                            .and_then(|v| v.as_str().map(|s| s.to_string())),
                                        formatted
                                            .get("hierarchy_radio_lvl2")
                                            .and_then(|v| v.as_str().map(|s| s.to_string())),
                                        formatted
                                            .get("hierarchy_radio_lvl3")
                                            .and_then(|v| v.as_str().map(|s| s.to_string())),
                                        formatted
                                            .get("hierarchy_radio_lvl4")
                                            .and_then(|v| v.as_str().map(|s| s.to_string())),
                                        formatted
                                            .get("hierarchy_radio_lvl5")
                                            .and_then(|v| v.as_str().map(|s| s.to_string())),
                                    ],
                                    hierarchy: [
                                        formatted
                                            .get("hierarchy_lvl0")
                                            .and_then(|v| v.as_str().map(|s| s.to_string())),
                                        formatted
                                            .get("hierarchy_lvl1")
                                            .and_then(|v| v.as_str().map(|s| s.to_string())),
                                        formatted
                                            .get("hierarchy_lvl2")
                                            .and_then(|v| v.as_str().map(|s| s.to_string())),
                                        formatted
                                            .get("hierarchy_lvl3")
                                            .and_then(|v| v.as_str().map(|s| s.to_string())),
                                        formatted
                                            .get("hierarchy_lvl4")
                                            .and_then(|v| v.as_str().map(|s| s.to_string())),
                                        formatted
                                            .get("hierarchy_lvl5")
                                            .and_then(|v| v.as_str().map(|s| s.to_string())),
                                    ],
                                    content: formatted
                                        .get("content")
                                        .and_then(|v| v.as_str().map(|s| s.to_string())),
                                    object_id: hit.result.object_id,
                                    rfd_number: hit.result.rfd_number,
                                    anchor: formatted
                                        .get("anchor")
                                        .and_then(|v| v.as_str().map(|s| s.to_string())),
                                    url: formatted
                                        .get("url")
                                        .and_then(|v| v.as_str().map(|s| s.to_string())),
                                }
                            }),
                        })
                        .collect::<Vec<_>>(),
                    query: results.query,
                    limit: results.limit,
                    offset: results.offset,
                };

                tracing::debug!(count = ?results.hits.len(), elapsed = ?timer.mark(), "Transformed search results");

                Ok(HttpResponseOk(results))
            }
            Err(err) => {
                tracing::error!(?err, "Search request failed");
                Err(internal_error("Search failed".to_string()))
            }
        }
    } else {
        Err(unauthorized())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use chrono::Utc;
    use dropshot::HttpResponseOk;
    use http::StatusCode;
    use rfd_model::{
        permissions::Caller,
        storage::{MockRfdPdfStore, MockRfdRevisionStore, MockRfdStore},
        Rfd, RfdRevision,
    };
    use uuid::Uuid;

    use crate::{
        context::{
            test_mocks::{mock_context, MockStorage},
            ApiContext,
        },
        endpoints::rfd::get_rfd_op,
        permissions::ApiPermission,
    };

    use super::get_rfds_op;

    async fn ctx() -> ApiContext {
        let private_rfd_id_1 = Uuid::new_v4();
        let private_rfd_id_2 = Uuid::new_v4();
        let public_rfd_id = Uuid::new_v4();

        let mut rfd_store = MockRfdStore::new();
        rfd_store.expect_list().returning(move |filter, _| {
            let mut results = vec![
                Rfd {
                    id: private_rfd_id_1,
                    rfd_number: 123,
                    link: None,
                    created_at: Utc::now(),
                    updated_at: Utc::now(),
                    deleted_at: None,
                    visibility: rfd_model::schema_ext::Visibility::Private,
                },
                Rfd {
                    id: public_rfd_id,
                    rfd_number: 456,
                    link: None,
                    created_at: Utc::now(),
                    updated_at: Utc::now(),
                    deleted_at: None,
                    visibility: rfd_model::schema_ext::Visibility::Public,
                },
                Rfd {
                    id: private_rfd_id_2,
                    rfd_number: 789,
                    link: None,
                    created_at: Utc::now(),
                    updated_at: Utc::now(),
                    deleted_at: None,
                    visibility: rfd_model::schema_ext::Visibility::Private,
                },
            ];

            results.retain(|rfd| {
                filter.rfd_number.is_none()
                    || filter
                        .rfd_number
                        .as_ref()
                        .unwrap()
                        .contains(&rfd.rfd_number)
            });

            Ok(results)
        });

        let mut rfd_revision_store = MockRfdRevisionStore::new();
        rfd_revision_store
            .expect_list()
            .returning(move |filter, _| {
                let mut results = vec![
                    RfdRevision {
                        id: Uuid::new_v4(),
                        rfd_id: private_rfd_id_1,
                        title: "Private Test RFD 1".to_string(),
                        state: None,
                        discussion: None,
                        authors: None,
                        content: String::new(),
                        content_format: rfd_model::schema_ext::ContentFormat::Asciidoc,
                        sha: String::new(),
                        commit_sha: String::new(),
                        committed_at: Utc::now(),
                        created_at: Utc::now(),
                        updated_at: Utc::now(),
                        deleted_at: None,
                    },
                    RfdRevision {
                        id: Uuid::new_v4(),
                        rfd_id: public_rfd_id,
                        title: "Public Test RFD".to_string(),
                        state: None,
                        discussion: None,
                        authors: None,
                        content: String::new(),
                        content_format: rfd_model::schema_ext::ContentFormat::Asciidoc,
                        sha: String::new(),
                        commit_sha: String::new(),
                        committed_at: Utc::now(),
                        created_at: Utc::now(),
                        updated_at: Utc::now(),
                        deleted_at: None,
                    },
                    RfdRevision {
                        id: Uuid::new_v4(),
                        rfd_id: private_rfd_id_2,
                        title: "Private Test RFD 2".to_string(),
                        state: None,
                        discussion: None,
                        authors: None,
                        content: String::new(),
                        content_format: rfd_model::schema_ext::ContentFormat::Asciidoc,
                        sha: String::new(),
                        commit_sha: String::new(),
                        committed_at: Utc::now(),
                        created_at: Utc::now(),
                        updated_at: Utc::now(),
                        deleted_at: None,
                    },
                ];

                results.retain(|revision| {
                    filter.rfd.is_none() || filter.rfd.as_ref().unwrap().contains(&revision.rfd_id)
                });

                Ok(results)
            });

        let mut rfd_pdf_store = MockRfdPdfStore::new();
        rfd_pdf_store
            .expect_list()
            .returning(move |_, _| Ok(vec![]));

        let mut storage = MockStorage::new();
        storage.rfd_store = Some(Arc::new(rfd_store));
        storage.rfd_revision_store = Some(Arc::new(rfd_revision_store));
        storage.rfd_pdf_store = Some(Arc::new(rfd_pdf_store));

        mock_context(storage).await
    }

    // Test RFD access via the global All RFDs permission

    #[tokio::test]
    async fn list_rfds_via_all_permission() {
        let ctx = ctx().await;
        let caller = Caller {
            id: Uuid::new_v4(),
            permissions: vec![ApiPermission::GetRfdsAll].into(),
        };

        let HttpResponseOk(rfds) = get_rfds_op(&ctx, &caller).await.unwrap();
        assert_eq!(3, rfds.len());
        assert_eq!(789, rfds[0].rfd_number);
        assert_eq!(456, rfds[1].rfd_number);
        assert_eq!(123, rfds[2].rfd_number);
    }

    #[tokio::test]
    async fn get_rfd_via_all_permission() {
        let ctx = ctx().await;
        let caller = Caller {
            id: Uuid::new_v4(),
            permissions: vec![ApiPermission::GetRfdsAll].into(),
        };

        let HttpResponseOk(rfd) = get_rfd_op(&ctx, &caller, "0123".to_string()).await.unwrap();
        assert_eq!(123, rfd.rfd_number);

        let HttpResponseOk(rfd) = get_rfd_op(&ctx, &caller, "0456".to_string()).await.unwrap();
        assert_eq!(456, rfd.rfd_number);
    }

    // Test RFD access via the direct permission to an RFD

    #[tokio::test]
    async fn list_rfds_with_direct_permission() {
        let ctx = ctx().await;
        let caller = Caller {
            id: Uuid::new_v4(),
            permissions: vec![ApiPermission::GetRfd(123)].into(),
        };

        let HttpResponseOk(rfds) = get_rfds_op(&ctx, &caller).await.unwrap();
        assert_eq!(2, rfds.len());
        assert_eq!(456, rfds[0].rfd_number);
        assert_eq!(123, rfds[1].rfd_number);
    }

    #[tokio::test]
    async fn get_rfd_with_direct_permission() {
        let ctx = ctx().await;
        let caller = Caller {
            id: Uuid::new_v4(),
            permissions: vec![ApiPermission::GetRfd(123)].into(),
        };

        let HttpResponseOk(rfd) = get_rfd_op(&ctx, &caller, "0123".to_string()).await.unwrap();
        assert_eq!(123, rfd.rfd_number);

        let HttpResponseOk(rfd) = get_rfd_op(&ctx, &caller, "0456".to_string()).await.unwrap();
        assert_eq!(456, rfd.rfd_number);
    }

    // Test RFD access fails when a caller does not have permission

    #[tokio::test]
    async fn list_rfds_without_permission() {
        let ctx = ctx().await;
        let caller = Caller {
            id: Uuid::new_v4(),
            permissions: vec![].into(),
        };

        let HttpResponseOk(rfds) = get_rfds_op(&ctx, &caller).await.unwrap();
        assert_eq!(1, rfds.len());
        assert_eq!(456, rfds[0].rfd_number);
    }

    #[tokio::test]
    async fn get_rfd_without_permission() {
        let ctx = ctx().await;
        let caller = Caller {
            id: Uuid::new_v4(),
            permissions: vec![].into(),
        };

        let result = get_rfd_op(&ctx, &caller, "0123".to_string()).await;

        match result {
            Err(err) => assert_eq!(StatusCode::NOT_FOUND, err.status_code),
            Ok(response) => panic!(
                "Expected a 404 error, but instead found an RFD {:?}",
                response.0
            ),
        }

        let HttpResponseOk(rfd) = get_rfd_op(&ctx, &caller, "0456".to_string()).await.unwrap();
        assert_eq!(456, rfd.rfd_number);
    }

    // Test RFD access to public RFDs as the unauthenticated user

    #[tokio::test]
    async fn list_rfds_as_unauthenticated() {
        let ctx = ctx().await;

        let HttpResponseOk(rfds) = get_rfds_op(&ctx, &ctx.get_unauthenticated_caller())
            .await
            .unwrap();
        assert_eq!(1, rfds.len());
        assert_eq!(456, rfds[0].rfd_number);
    }

    #[tokio::test]
    async fn get_rfd_as_unauthenticated() {
        let ctx = ctx().await;
        let caller = ctx.get_unauthenticated_caller();

        let result = get_rfd_op(&ctx, caller, "0123".to_string()).await;
        match result {
            Err(err) => assert_eq!(StatusCode::NOT_FOUND, err.status_code),
            Ok(response) => panic!(
                "Expected a 404 error, but instead found an RFD {:?}",
                response.0
            ),
        }

        let HttpResponseOk(rfd) = get_rfd_op(&ctx, caller, "0456".to_string()).await.unwrap();
        assert_eq!(456, rfd.rfd_number);
    }
}
