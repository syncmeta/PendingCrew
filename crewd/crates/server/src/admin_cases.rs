//! Human inspection and explicit proposals for durable administrative custody.
//! Reads are passive; a proposal does not accept responsibility or admit sessions.
use crate::{AppState, dispatch, public_commands::Actor};
use crewd_proto::{Request, Response, RpcError, admin_cases::*};
use std::sync::Arc;

pub(crate) async fn handle(state: &Arc<AppState>, actor: &Actor, req: Request) -> Response {
    let id = req.id;
    match route(state, actor, req).await {
        Ok(value) => Response::ok(id, value),
        Err(error) => Response::err(id, error),
    }
}

async fn route(
    state: &Arc<AppState>,
    actor: &Actor,
    req: Request,
) -> Result<serde_json::Value, RpcError> {
    actor.authorize(state, &req.method).await?;
    let owner = state.clone();
    let by = actor.clone();
    match req.method.as_str() {
        WORK_DUPLICATE_HISTORY => {
            let p: crewd_proto::WorkDuplicateHistoryParams = crewd_proto::parse_params(req.params)?;
            let page = state.db.call_result(move |store| {
                by.authorize_tx(&owner, store.conn(), WORK_DUPLICATE_HISTORY)?;
                crewd_collab::work_duplicate_review::reviewed_history(store.conn(), p)
            }).await.map_err(dispatch::internal)?.map_err(dispatch::collab_err)?;
            serde_json::to_value(page).map_err(dispatch::internal)
        }
        WORK_DUPLICATES => {
            let p: crewd_proto::WorkDuplicatesParams = crewd_proto::parse_params(req.params)?;
            let page = state.db.call_result(move |store| {
                by.authorize_tx(&owner, store.conn(), WORK_DUPLICATES)?;
                crewd_collab::work_duplicate_review::list(store.conn(), &p)
            }).await.map_err(dispatch::internal)?.map_err(dispatch::collab_err)?;
            serde_json::to_value(page).map_err(dispatch::internal)
        }
        REQUIREMENT_REVIEWS => {
            let p: RequirementReviewsParams = crewd_proto::parse_params(req.params)?;
            let page = state.db.call_result(move |store| {
                by.authorize_tx(&owner, store.conn(), REQUIREMENT_REVIEWS)?;
                crewd_collab::requirement_review::list(store.conn(), &p)
            }).await.map_err(dispatch::internal)?.map_err(dispatch::collab_err)?;
            serde_json::to_value(page).map_err(dispatch::internal)
        }
        SIMILAR_REQUIREMENTS => {
            let p: SimilarRequirementsParams = crewd_proto::parse_params(req.params)?;
            let view = state
                .db
                .call_result(move |store| {
                    by.authorize_tx(&owner, store.conn(), SIMILAR_REQUIREMENTS)?;
                    crewd_collab::requirement_similarity::list(
                        store.conn(),
                        &p.crew_id,
                        p.after,
                        p.limit.unwrap_or(20),
                    )
                })
                .await
                .map_err(dispatch::internal)?
                .map_err(dispatch::collab_err)?;
            serde_json::to_value(view).map_err(dispatch::internal)
        }
        LEGACY_LIST => {
            let p: AdminCaseLegacyListParams = crewd_proto::parse_params(req.params)?;
            let view = state
                .db
                .call_result(move |store| {
                    by.authorize_tx(&owner, store.conn(), LEGACY_LIST)?;
                    crewd_collab::admin_case_legacy::list(
                        store.conn(),
                        &crewd_collab::Sender::Human,
                        p.after,
                        p.limit.unwrap_or(50),
                    )
                })
                .await
                .map_err(dispatch::internal)?
                .map_err(dispatch::collab_err)?;
            serde_json::to_value(view).map_err(dispatch::internal)
        }
        LEGACY_GET => {
            let p: AdminCaseLegacyGetParams = crewd_proto::parse_params(req.params)?;
            let view = state
                .db
                .call_result(move |store| {
                    by.authorize_tx(&owner, store.conn(), LEGACY_GET)?;
                    crewd_collab::admin_case_legacy::get(
                        store.conn(),
                        &crewd_collab::Sender::Human,
                        &p.message_id,
                    )
                })
                .await
                .map_err(dispatch::internal)?
                .map_err(dispatch::collab_err)?;
            serde_json::to_value(view).map_err(dispatch::internal)
        }
        LEGACY_PREVIEW => {
            let p: AdminCaseLegacyPreviewParams = crewd_proto::parse_params(req.params)?;
            let view = state
                .db
                .call_result(move |store| {
                    by.authorize_tx(&owner, store.conn(), LEGACY_PREVIEW)?;
                    crewd_collab::admin_case_legacy::preview(
                        store.conn(),
                        &crewd_collab::Sender::Human,
                        &p,
                        crewd_store::now_ms(),
                    )
                })
                .await
                .map_err(dispatch::internal)?
                .map_err(dispatch::collab_err)?;
            serde_json::to_value(view).map_err(dispatch::internal)
        }
        LEGACY_CONFIRM => {
            let p: AdminCaseLegacyConfirmParams = crewd_proto::parse_params(req.params)?;
            let view = state
                .collab(move |tx| {
                    by.authorize_tx(&owner, tx.conn(), LEGACY_CONFIRM)?;
                    // Confirmation records current Human revalidation, not session
                    // admission and not a replacement for the native judgment.
                    crewd_collab::admin_case_legacy::confirm(tx, &crewd_collab::Sender::Human, p)
                })
                .await
                .map_err(dispatch::collab_err)?;
            serde_json::to_value(view).map_err(dispatch::internal)
        }
        LEGACY_RECEIPT => {
            let p: AdminCaseLegacyReceiptParams = crewd_proto::parse_params(req.params)?;
            let view = state
                .db
                .call_result(move |store| {
                    by.authorize_tx(&owner, store.conn(), LEGACY_RECEIPT)?;
                    crewd_collab::admin_case_legacy::receipt(
                        store.conn(),
                        &crewd_collab::Sender::Human,
                        &p.request_id,
                    )
                })
                .await
                .map_err(dispatch::internal)?
                .map_err(dispatch::collab_err)?;
            serde_json::to_value(view).map_err(dispatch::internal)
        }
        crewd_proto::methods::ADMIN_CASE_CUSTODY_TRANSFER_GET => {
            let p: AdminCaseCustodyTransferGetParams = crewd_proto::parse_params(req.params)?;
            let view = state
                .db
                .call_result(move |store| {
                    by.authorize_tx(
                        &owner,
                        store.conn(),
                        crewd_proto::methods::ADMIN_CASE_CUSTODY_TRANSFER_GET,
                    )?;
                    crewd_collab::admin_case_custody::get_transfer(
                        store.conn(),
                        &crewd_collab::Sender::Human,
                        &p.request_id,
                        crewd_store::now_ms(),
                    )
                })
                .await
                .map_err(dispatch::internal)?
                .map_err(dispatch::collab_err)?;
            serde_json::to_value(view).map_err(dispatch::internal)
        }
        crewd_proto::methods::ADMIN_CASE_CUSTODY_GET => {
            let p: AdminCaseCustodyGetParams = crewd_proto::parse_params(req.params)?;
            let view = state
                .db
                .call_result(move |store| {
                    by.authorize_tx(
                        &owner,
                        store.conn(),
                        crewd_proto::methods::ADMIN_CASE_CUSTODY_GET,
                    )?;
                    crewd_collab::admin_case_custody::get(
                        store.conn(),
                        &crewd_collab::Sender::Human,
                        &p.crew_id,
                        &p.case_id,
                        crewd_store::now_ms(),
                    )
                })
                .await
                .map_err(dispatch::internal)?
                .map_err(dispatch::collab_err)?;
            serde_json::to_value(view).map_err(dispatch::internal)
        }
        crewd_proto::methods::ADMIN_CASE_CUSTODY_PROPOSE => {
            let p: AdminCaseCustodyPropose = crewd_proto::parse_params(req.params)?;
            let view = state
                .collab(move |tx| {
                    by.authorize_tx(
                        &owner,
                        tx.conn(),
                        crewd_proto::methods::ADMIN_CASE_CUSTODY_PROPOSE,
                    )?;
                    let captain = crewd_collab::crews::captain_of(tx.conn(), &p.crew_id)?
                        .ok_or_else(|| {
                            crewd_collab::resource_invalid("custody captain unavailable")
                        })?;
                    if !owner.admission.contains(&captain) {
                        return Err(crewd_collab::resource_invalid(
                            "custody captain admission required",
                        ));
                    }
                    crewd_collab::admin_case_custody::propose(tx, &crewd_collab::Sender::Human, p)
                })
                .await
                .map_err(dispatch::collab_err)?;
            serde_json::to_value(view).map_err(dispatch::internal)
        }
        LIST => {
            let p: AdminCaseListParams = crewd_proto::parse_params(req.params)?;
            let view = state
                .db
                .call_result(move |store| {
                    by.authorize_tx(&owner, store.conn(), LIST)?;
                    crewd_collab::admin_cases::list_at(store.conn(), &p, crewd_store::now_ms())
                })
                .await
                .map_err(dispatch::internal)?
                .map_err(dispatch::collab_err)?;
            serde_json::to_value(view).map_err(dispatch::internal)
        }
        HUMAN_BRIEF => {
            let p: AdminHumanBriefParams = crewd_proto::parse_params(req.params)?;
            let brief = state
                .db
                .call_result(move |store| {
                    by.authorize_tx(&owner, store.conn(), HUMAN_BRIEF)?;
                    crewd_collab::admin_human_brief::get(store.conn(), &p.crew_id, crewd_store::now_ms())
                })
                .await
                .map_err(dispatch::internal)?
                .map_err(dispatch::collab_err)?;
            serde_json::to_value(brief).map_err(dispatch::internal)
        }
        EFFECTS => {
            let p: AdminCaseEffectsParams = crewd_proto::parse_params(req.params)?;
            let page = state
                .db
                .call_result(move |store| {
                    by.authorize_tx(&owner, store.conn(), EFFECTS)?;
                    crewd_collab::admin_case_effect_report::list(store.conn(), &p)
                })
                .await
                .map_err(dispatch::internal)?
                .map_err(dispatch::collab_err)?;
            serde_json::to_value(page).map_err(dispatch::internal)
        }
        GET => {
            let p: AdminCaseGetParams = crewd_proto::parse_params(req.params)?;
            let view = state
                .db
                .call_result(move |store| {
                    by.authorize_tx(&owner, store.conn(), GET)?;
                    crewd_collab::admin_cases::get_at(store.conn(), &p, crewd_store::now_ms())
                })
                .await
                .map_err(dispatch::internal)?
                .map_err(dispatch::collab_err)?;
            serde_json::to_value(view).map_err(dispatch::internal)
        }
        _ => Err(RpcError::new(
            RpcError::METHOD_NOT_FOUND,
            "Unknown administrative record method",
        )),
    }
}
