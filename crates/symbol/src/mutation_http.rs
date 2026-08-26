#![allow(clippy::result_large_err)]

use std::fmt::Write as _;
use std::io;
use std::path::PathBuf;

use axum::Json;
use axum::body::{Body, to_bytes};
use axum::extract::{OriginalUri, Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use futures_util::StreamExt as _;
use tokio::io::AsyncWriteExt as _;

use super::{
    App, ExpiryRequest, TemporaryUpload, expiry_policy_from, has_expiry_parameters, if_match_from,
    insert_sanitized_headers, insert_undo_headers, management_bearer, plain,
};
use crate::secrets::ManagementToken;
use crate::splice::{self, ProtocolError};
use crate::store::{
    AliasEntry, AliasResolvedKind, AliasSpec, AllocatedFile, AllocatedName, AllocationSource,
    AllocationSpec, FileExpiry, FileMutationOptions, Idempotency, MutationResult,
    PendingAllocationSpec, Store, StoreError,
};
use symbol_contract as contract;

const MAX_ALIAS_BATCH_BYTES: usize = 1024 * 1024;
const MAX_ALIAS_BATCH_ENTRIES: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AllocationAction {
    Create,
    Propose,
    Finalize,
    Cancel,
}

struct MutationRequest {
    expected_tree_hash: Option<String>,
    idempotency: Idempotency,
    authorization: Option<ManagementToken>,
}

struct GeneratedNaming {
    prefix: String,
    suffix: String,
    extension: Option<String>,
}

pub async fn allocate_root(
    State(app): State<App>,
    Path(name): Path<String>,
    headers: HeaderMap,
    body: Body,
) -> Response {
    allocate(&app, name, String::new(), &headers, body).await
}

pub async fn reject_control_allocation(
    State(app): State<App>,
    Path(name): Path<String>,
    headers: HeaderMap,
    _body: Body,
) -> Response {
    reject_control_mutation(&app, &name, &headers).await
}

pub async fn reject_control_mutation(app: &App, name: &str, headers: &HeaderMap) -> Response {
    if let Err(response) = mutation_request(app, name, headers).await {
        return response;
    }
    plain(StatusCode::BAD_REQUEST, contract::RESERVED_MUTATION_ERROR)
}

pub async fn allocate_path(
    State(app): State<App>,
    Path((name, path)): Path<(String, String)>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    body: Body,
) -> Response {
    if !uri.path().ends_with('/') && crate::store::validate_mutation_target(&path).is_ok() {
        return plain(
            StatusCode::METHOD_NOT_ALLOWED,
            "error: allocated file target must end in /",
        );
    }
    allocate(
        &app,
        name,
        path.trim_end_matches('/').to_string(),
        &headers,
        body,
    )
    .await
}

pub async fn allocate_files_path(
    State(app): State<App>,
    Path((name, path)): Path<(String, String)>,
    headers: HeaderMap,
    body: Body,
) -> Response {
    allocate(
        &app,
        name,
        format!("FILES/{}", path.trim_end_matches('/')),
        &headers,
        body,
    )
    .await
}

async fn allocate(
    app: &App,
    name: String,
    folder: String,
    headers: &HeaderMap,
    body: Body,
) -> Response {
    let request = match mutation_request(app, &name, headers).await {
        Ok(request) => request,
        Err(response) => return response,
    };
    if let Err(error) = crate::store::validate_mutation_target(&folder) {
        return error.into_response();
    }
    let action = match allocation_action(headers) {
        Ok(action) => action,
        Err(response) => return response,
    };
    match action {
        AllocationAction::Create => allocate_fast(app, name, folder, headers, body, request).await,
        AllocationAction::Propose => {
            propose_allocation(app, name, folder, headers, body, request).await
        }
        AllocationAction::Finalize => {
            finalize_allocation(app, name, folder, headers, body, request).await
        }
        AllocationAction::Cancel => {
            cancel_allocation(app, name, folder, headers, body, request).await
        }
    }
}

async fn allocate_fast(
    app: &App,
    name: String,
    folder: String,
    headers: &HeaderMap,
    body: Body,
    request: MutationRequest,
) -> Response {
    if headers.contains_key("file-name") {
        return plain(
            StatusCode::BAD_REQUEST,
            "error: File-Name is only valid when finalizing an allocation",
        );
    }
    let naming = match generated_naming(headers) {
        Ok(naming) => naming,
        Err(response) => return response,
    };
    let media_type = match media_type(headers) {
        Ok(media_type) => media_type,
        Err(response) => return response,
    };
    let expiry = match allocation_expiry(headers, app.store.expiry_defaults(), FileExpiry::Clear) {
        Ok(expiry) => expiry,
        Err(response) => return response,
    };
    let temporary = match spool_content(app, headers, body, app.max_file_size).await {
        Ok(temporary) => temporary,
        Err(response) => return response,
    };
    let idempotency_key = request.idempotency.key.clone();
    let store_idempotency = request.idempotency.clone();
    let store_authorization = request.authorization.clone();
    let store_expected_tree_hash = request.expected_tree_hash.clone();
    let result = app
        .run_store({
            let source = temporary.path.clone();
            let folder = folder.clone();
            let prefix = naming.prefix.clone();
            let suffix = naming.suffix.clone();
            let extension = naming.extension.clone();
            let media_type = media_type.clone();
            let name = name.clone();
            move |store| {
                store.allocate_file(
                    &name,
                    &source,
                    AllocationSpec {
                        folder: &folder,
                        naming: AllocatedName {
                            prefix: &prefix,
                            suffix: &suffix,
                            extension: extension.as_deref(),
                        },
                        media_type: &media_type,
                    },
                    FileMutationOptions {
                        expected_tree_hash: store_expected_tree_hash.as_deref(),
                        idempotency: Some(&store_idempotency),
                        authorization: store_authorization.as_ref(),
                        expiry,
                    },
                )
            }
        })
        .await;
    let allocated = match result {
        Ok(allocated) => allocated,
        Err(error) => return mutation_error(app, &name, error).await,
    };
    allocated_response(
        app,
        &name,
        allocated,
        idempotency_key,
        contract::AllocationNaming::Generated {
            prefix: naming.prefix,
            extension: naming.extension.unwrap_or_default(),
            suffix: naming.suffix,
        },
    )
    .await
}

async fn propose_allocation(
    app: &App,
    name: String,
    folder: String,
    headers: &HeaderMap,
    body: Body,
    request: MutationRequest,
) -> Response {
    if headers.contains_key("file-name")
        || headers.contains_key("file-prefix")
        || headers.contains_key("file-suffix")
    {
        return plain(
            StatusCode::BAD_REQUEST,
            "error: proposal accepts only File-Extension as a naming hint",
        );
    }
    let media_type = match media_type(headers) {
        Ok(media_type) => media_type,
        Err(response) => return response,
    };
    let inferred_extension = match optional_header(headers, "file-extension", "File-Extension") {
        Ok(Some(extension)) => match crate::store::normalize_allocated_extension(&extension) {
            Ok(extension) => Some(extension),
            Err(error) => return error.into_response(),
        },
        Ok(None) => infer_extension(&media_type).map(str::to_string),
        Err(response) => return response,
    };
    let expiry = match allocation_expiry(headers, app.store.expiry_defaults(), FileExpiry::Clear) {
        Ok(expiry) => expiry,
        Err(response) => return response,
    };
    let temporary = match spool_content(app, headers, body, app.max_file_size).await {
        Ok(temporary) => temporary,
        Err(response) => return response,
    };
    let idempotency_key = request.idempotency.key.clone();
    let store_idempotency = request.idempotency.clone();
    let store_authorization = request.authorization.clone();
    let store_expected_tree_hash = request.expected_tree_hash.clone();
    let store_extension = inferred_extension.clone();
    let result = app
        .run_store({
            let source = temporary.path.clone();
            let name = name.clone();
            let folder = folder.clone();
            let media_type = media_type.clone();
            move |store| {
                store.propose_allocation_idempotent(
                    &name,
                    AllocationSource::File(&source),
                    PendingAllocationSpec {
                        folder: &folder,
                        media_type: &media_type,
                        extension: store_extension.as_deref(),
                    },
                    FileMutationOptions {
                        expected_tree_hash: store_expected_tree_hash.as_deref(),
                        idempotency: Some(&store_idempotency),
                        authorization: store_authorization.as_ref(),
                        expiry,
                    },
                )
            }
        })
        .await;
    let pending = match result {
        Ok(pending) => pending,
        Err(error) => return mutation_error(app, &name, error).await,
    };
    let replayed = pending.replayed;
    let proposal_tree_hash = pending.tree_hash.clone();
    let proposal_revision = pending.content_revision;
    let inferred_extension = pending.extension.clone();
    let default_name = inferred_extension.as_ref().map_or_else(
        || pending.hash.clone(),
        |extension| format!("{}.{}", pending.hash, extension),
    );
    let receipt = contract::AllocationProposalReceipt {
        allocation_token: pending.token,
        expires_at: pending.expires_at,
        proposal: contract::ProposedFileName {
            folder: pending.folder,
            default_name,
            hash: prefixed_hash(&pending.hash),
            size: pending.size,
            media_type: pending.media_type,
            inferred_extension,
        },
        idempotency_key,
        replayed,
    };
    let location = site_location(app, &name);
    let mut response = (StatusCode::ACCEPTED, Json(receipt)).into_response();
    insert_snapshot_values(
        response.headers_mut(),
        &location,
        &proposal_tree_hash,
        proposal_revision,
    );
    insert_replay_header(response.headers_mut(), replayed);
    response
}

async fn finalize_allocation(
    app: &App,
    name: String,
    folder: String,
    headers: &HeaderMap,
    body: Body,
    request: MutationRequest,
) -> Response {
    if let Err(response) = require_empty_body(body).await {
        return response;
    }
    let token = match required_text_header(headers, "allocation-token", "Allocation-Token") {
        Ok(token) => token,
        Err(response) => return response,
    };
    let basename = match required_text_header(headers, "file-name", "File-Name") {
        Ok(name) => name,
        Err(response) => return response,
    };
    if ["file-prefix", "file-extension", "file-suffix"]
        .iter()
        .any(|header| headers.contains_key(*header))
    {
        return plain(
            StatusCode::BAD_REQUEST,
            "error: generated naming headers cannot be combined with File-Name",
        );
    }
    let expiry = match allocation_expiry(headers, app.store.expiry_defaults(), FileExpiry::Preserve)
    {
        Ok(expiry) => expiry,
        Err(response) => return response,
    };
    let idempotency_key = request.idempotency.key.clone();
    let store_idempotency = request.idempotency.clone();
    let store_authorization = request.authorization.clone();
    let store_expected_tree_hash = request.expected_tree_hash.clone();
    let result = app
        .run_store({
            let name = name.clone();
            let token = token.clone();
            let basename = basename.clone();
            let folder = folder.clone();
            move |store| {
                store.finalize_allocation_custom_in_folder(
                    &name,
                    &token,
                    &folder,
                    &basename,
                    FileMutationOptions {
                        expected_tree_hash: store_expected_tree_hash.as_deref(),
                        idempotency: Some(&store_idempotency),
                        authorization: store_authorization.as_ref(),
                        expiry,
                    },
                )
            }
        })
        .await;
    let allocated = match result {
        Ok(allocated) => allocated,
        Err(error) => return mutation_error(app, &name, error).await,
    };
    allocated_response(
        app,
        &name,
        allocated,
        idempotency_key,
        contract::AllocationNaming::Custom,
    )
    .await
}

async fn cancel_allocation(
    app: &App,
    name: String,
    folder: String,
    headers: &HeaderMap,
    body: Body,
    request: MutationRequest,
) -> Response {
    if let Err(response) = require_empty_body(body).await {
        return response;
    }
    let token = match required_text_header(headers, "allocation-token", "Allocation-Token") {
        Ok(token) => token,
        Err(response) => return response,
    };
    let result = app
        .run_store({
            let name = name.clone();
            let token = token.clone();
            let folder = folder.clone();
            let authorization = request.authorization.clone();
            let idempotency = request.idempotency.clone();
            let expected_tree_hash = request.expected_tree_hash.clone();
            move |store| {
                store.cancel_allocation_idempotent_for_folder(
                    &name,
                    &token,
                    &folder,
                    FileMutationOptions {
                        expected_tree_hash: expected_tree_hash.as_deref(),
                        idempotency: Some(&idempotency),
                        authorization: authorization.as_ref(),
                        expiry: FileExpiry::Preserve,
                    },
                )
            }
        })
        .await;
    let cancellation = match result {
        Ok(cancelled) => cancelled,
        Err(error) => return error.into_response(),
    };
    let replayed = cancellation.replayed;
    let receipt = contract::AllocationCancellationReceipt {
        allocation_token: token,
        cancelled: true,
        idempotency_key: request.idempotency.key,
        replayed,
    };
    let location = site_location(app, &name);
    let mut response = (StatusCode::OK, Json(receipt)).into_response();
    insert_snapshot_values(
        response.headers_mut(),
        &location,
        &cancellation.tree_hash,
        cancellation.content_revision,
    );
    if replayed {
        response
            .headers_mut()
            .insert("idempotency-replayed", HeaderValue::from_static("true"));
    }
    response
}

pub async fn alias_file(app: &App, name: &str, path: &str, headers: &HeaderMap) -> Response {
    let request = match mutation_request(app, name, headers).await {
        Ok(request) => request,
        Err(response) => return response,
    };
    if let Err(error) = crate::store::validate_mutation_target(path) {
        return error.into_response();
    }
    let target = match required_text_header(headers, "alias-target", "Alias-Target") {
        Ok(target) => target,
        Err(response) => return response,
    };
    if target.len() > crate::upload::MAX_ALIAS_TARGET_BYTES {
        return plain(
            StatusCode::PAYLOAD_TOO_LARGE,
            "error: Alias-Target exceeds the configured limit",
        );
    }
    let idempotency_key = request.idempotency.key.clone();
    let result = app
        .run_store({
            let name = name.to_string();
            let path = path.to_string();
            move |store| {
                let result = store.put_aliases_with_receipt(
                    &name,
                    &[AliasSpec {
                        path: &path,
                        target: &target,
                    }],
                    FileMutationOptions {
                        expected_tree_hash: request.expected_tree_hash.as_deref(),
                        idempotency: Some(&request.idempotency),
                        authorization: request.authorization.as_ref(),
                        expiry: FileExpiry::Preserve,
                    },
                )?;
                Ok(result)
            }
        })
        .await;
    let result = match result {
        Ok(result) => result,
        Err(error) => return mutation_error(app, name, error).await,
    };
    let mutation = result.mutation;
    let alias = result
        .aliases
        .into_iter()
        .next()
        .expect("single alias mutation stores one receipt row");
    let location = resource_location(app, name, &alias.path);
    let receipt = alias_receipt(
        alias,
        mutation_receipt(&mutation, &idempotency_key, location.clone()),
    );
    let status = if mutation.created {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    };
    let mut response = (status, Json(receipt)).into_response();
    insert_mutation_headers(response.headers_mut(), &mutation, &location);
    response
}

pub async fn alias_batch(app: &App, name: &str, headers: &HeaderMap, body: Body) -> Response {
    let request = match mutation_request(app, name, headers).await {
        Ok(request) => request,
        Err(response) => return response,
    };
    if !content_type_is(headers, "application/json") {
        return plain(
            StatusCode::BAD_REQUEST,
            "error: alias batch Content-Type must be application/json",
        );
    }
    if content_length(headers).is_some_and(|length| length > MAX_ALIAS_BATCH_BYTES as u64) {
        return plain(
            StatusCode::PAYLOAD_TOO_LARGE,
            "error: alias batch is too large",
        );
    }
    let Ok(bytes) = to_bytes(body, MAX_ALIAS_BATCH_BYTES).await else {
        return plain(
            StatusCode::PAYLOAD_TOO_LARGE,
            "error: alias batch is too large",
        );
    };
    let batch = match parse_alias_batch(&bytes) {
        Ok(batch) => batch,
        Err(response) => return response,
    };
    if let Some(error) = batch
        .aliases
        .iter()
        .find_map(|alias| crate::store::validate_mutation_target(&alias.path).err())
    {
        return error.into_response();
    }
    let idempotency_key = request.idempotency.key.clone();
    let result = app
        .run_store({
            let name = name.to_string();
            move |store| {
                let specs = batch
                    .aliases
                    .iter()
                    .map(|alias| AliasSpec {
                        path: &alias.path,
                        target: &alias.target,
                    })
                    .collect::<Vec<_>>();
                store.put_aliases_with_receipt(
                    &name,
                    &specs,
                    FileMutationOptions {
                        expected_tree_hash: request.expected_tree_hash.as_deref(),
                        idempotency: Some(&request.idempotency),
                        authorization: request.authorization.as_ref(),
                        expiry: FileExpiry::Preserve,
                    },
                )
            }
        })
        .await;
    let result = match result {
        Ok(result) => result,
        Err(error) => return mutation_error(app, name, error).await,
    };
    let mutation = result.mutation;
    let aliases = result.aliases;
    let location = site_location(app, name);
    let receipt = contract::AliasBatchReceipt {
        aliases: aliases.into_iter().map(inventory_alias).collect(),
        mutation: mutation_receipt(&mutation, &idempotency_key, location.clone()),
    };
    let status = if mutation.created {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    };
    let mut response = (status, Json(receipt)).into_response();
    insert_mutation_headers(response.headers_mut(), &mutation, &location);
    response
}

pub async fn replace_file(
    app: &App,
    name: &str,
    path: &str,
    headers: &HeaderMap,
    body: Body,
) -> Response {
    let request = match mutation_request(app, name, headers).await {
        Ok(request) => request,
        Err(response) => return response,
    };
    if let Err(error) = crate::store::validate_mutation_target(path) {
        return error.into_response();
    }
    let base_hash = match content_match(headers) {
        Ok(hash) => hash,
        Err(response) => return response,
    };
    let temporary = match spool_content(app, headers, body, app.max_file_size).await {
        Ok(temporary) => temporary,
        Err(response) => return response,
    };
    let idempotency_key = request.idempotency.key.clone();
    let result = app
        .run_store({
            let name = name.to_string();
            let path = path.to_string();
            let base_hash = base_hash.clone();
            let source = temporary.path.clone();
            move |store| {
                store.replace_file_content(
                    &name,
                    &path,
                    &base_hash,
                    AllocationSource::File(&source),
                    FileMutationOptions {
                        expected_tree_hash: request.expected_tree_hash.as_deref(),
                        idempotency: Some(&request.idempotency),
                        authorization: request.authorization.as_ref(),
                        expiry: FileExpiry::Preserve,
                    },
                )
            }
        })
        .await;
    let replaced = match result {
        Ok(replaced) => replaced,
        Err(error) => return mutation_error(app, name, error).await,
    };
    let location = resource_location(app, name, &replaced.path);
    let relocated = replaced.path != path;
    let outcome = if !replaced.changed {
        contract::ReplacementOutcome::Unchanged
    } else if relocated {
        contract::ReplacementOutcome::Relocated
    } else {
        contract::ReplacementOutcome::Replaced
    };
    let mutation =
        match allocated_mutation_receipt(app, name, &replaced, &idempotency_key, location.clone())
            .await
        {
            Ok(mutation) => mutation,
            Err(error) => return error.into_response(),
        };
    let receipt = contract::FileReplaceReceipt {
        outcome,
        old_path: path.to_string(),
        new_path: replaced.path.clone(),
        relocated,
        old_hash: prefixed_hash(&base_hash),
        new_hash: prefixed_hash(&replaced.hash),
        size: replaced.size,
        mutation,
    };
    let mut response = (StatusCode::OK, Json(receipt)).into_response();
    if let Err(error) =
        insert_allocated_headers(app, name, &replaced, &location, response.headers_mut()).await
    {
        return error.into_response();
    }
    response
}

pub async fn splice_file(
    State(app): State<App>,
    Path((name, path)): Path<(String, String)>,
    headers: HeaderMap,
    body: Body,
) -> Response {
    splice_file_inner(app, name, path, headers, body).await
}

pub async fn splice_files_path(
    State(app): State<App>,
    Path((name, path)): Path<(String, String)>,
    headers: HeaderMap,
    body: Body,
) -> Response {
    splice_file_inner(app, name, format!("FILES/{path}"), headers, body).await
}

async fn splice_file_inner(
    app: App,
    name: String,
    path: String,
    headers: HeaderMap,
    body: Body,
) -> Response {
    let request = match mutation_request(&app, &name, &headers).await {
        Ok(request) => request,
        Err(response) => return response,
    };
    if let Err(error) = crate::store::validate_mutation_target(&path) {
        return error.into_response();
    }
    let base_hash = match content_match(&headers) {
        Ok(hash) => hash,
        Err(response) => return response,
    };
    let format = match splice::select_format(&headers) {
        Ok(format) => format,
        Err(error) => return splice_error(&error),
    };
    let parsed = match splice::parse(
        format,
        &headers,
        body,
        app.store.upload_path(),
        app.max_file_size,
    )
    .await
    {
        Ok(parsed) => parsed,
        Err(error) => return splice_error(&error),
    };
    let old_size = tokio::fs::metadata(app.store.blob_path(&base_hash))
        .await
        .ok()
        .map(|metadata| metadata.len());
    let maximum_result_size = app.max_file_size;
    let idempotency_key = request.idempotency.key.clone();
    let result = app
        .run_store({
            let name = name.clone();
            let path = path.clone();
            let base_hash = base_hash.clone();
            move |store| {
                let old_size = old_size.unwrap_or(0);
                let count = parsed.count();
                let splices = parsed.as_store_splices();
                let result = store.splice_file_with_limit(
                    &name,
                    &path,
                    &base_hash,
                    &splices,
                    FileMutationOptions {
                        expected_tree_hash: request.expected_tree_hash.as_deref(),
                        idempotency: Some(&request.idempotency),
                        authorization: request.authorization.as_ref(),
                        expiry: FileExpiry::Preserve,
                    },
                    maximum_result_size,
                )?;
                Ok((result, old_size, count))
            }
        })
        .await;
    let (spliced, old_size, count) = match result {
        Ok(result) => result,
        Err(error) => return mutation_error(&app, &name, error).await,
    };
    let location = resource_location(&app, &name, &spliced.path);
    let mutation =
        match allocated_mutation_receipt(&app, &name, &spliced, &idempotency_key, location.clone())
            .await
        {
            Ok(mutation) => mutation,
            Err(error) => return error.into_response(),
        };
    let receipt = contract::SpliceReceipt {
        old_path: path.clone(),
        new_path: spliced.path.clone(),
        relocated: spliced.path != path,
        old_hash: prefixed_hash(&base_hash),
        new_hash: prefixed_hash(&spliced.hash),
        old_size,
        new_size: spliced.size,
        splices: count,
        mutation,
    };
    let mut response = (StatusCode::OK, Json(receipt)).into_response();
    if let Err(error) =
        insert_allocated_headers(&app, &name, &spliced, &location, response.headers_mut()).await
    {
        return error.into_response();
    }
    response
}

async fn allocated_response(
    app: &App,
    name: &str,
    allocated: AllocatedFile,
    idempotency_key: String,
    naming: contract::AllocationNaming,
) -> Response {
    let url = resource_location(app, name, &allocated.path);
    let blob_url = format!("{}/.blob/{name}/{}", app.public_url, allocated.hash);
    let created = allocated
        .mutation
        .as_ref()
        .is_some_and(|mutation| mutation.created);
    let mutation = match allocated_mutation_receipt(
        app,
        name,
        &allocated,
        &idempotency_key,
        url.clone(),
    )
    .await
    {
        Ok(mutation) => mutation,
        Err(error) => return error.into_response(),
    };
    let receipt = contract::AllocatedFileReceipt {
        outcome: if created {
            contract::AllocationOutcome::Created
        } else {
            contract::AllocationOutcome::Existing
        },
        site: name.to_string(),
        name: allocated
            .path
            .rsplit('/')
            .next()
            .unwrap_or(&allocated.path)
            .to_string(),
        path: allocated.path.clone(),
        url: url.clone(),
        hash: prefixed_hash(&allocated.hash),
        size: allocated.size,
        blob_url: blob_url.clone(),
        naming,
        mutation,
    };
    let status = if created {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    };
    let mut response = (status, Json(receipt)).into_response();
    if let Err(error) =
        insert_allocated_headers(app, name, &allocated, &url, response.headers_mut()).await
    {
        return error.into_response();
    }
    response.headers_mut().insert(
        "content-location",
        HeaderValue::from_str(&blob_url).expect("valid blob URL"),
    );
    response
}

async fn mutation_request(
    app: &App,
    name: &str,
    headers: &HeaderMap,
) -> Result<MutationRequest, Response> {
    let authorization = management_bearer(headers).map_err(IntoResponse::into_response)?;
    let authorized = app
        .run_store({
            let name = name.to_string();
            let authorization = authorization.clone();
            move |store| store.authorize_mutation(&name, authorization.as_ref())
        })
        .await;
    if let Err(error) = authorized {
        return Err(error.into_response());
    }
    let expected_tree_hash =
        if_match_from(headers).map_err(|message| plain(StatusCode::BAD_REQUEST, message))?;
    let idempotency = idempotency(headers)?;
    Ok(MutationRequest {
        expected_tree_hash,
        idempotency,
        authorization,
    })
}

fn allocation_action(headers: &HeaderMap) -> Result<AllocationAction, Response> {
    match optional_header(headers, "allocation-action", "Allocation-Action")?.as_deref() {
        None | Some("create") => Ok(AllocationAction::Create),
        Some("propose") => Ok(AllocationAction::Propose),
        Some("finalize") => Ok(AllocationAction::Finalize),
        Some("cancel") => Ok(AllocationAction::Cancel),
        Some(_) => Err(plain(
            StatusCode::BAD_REQUEST,
            "error: invalid Allocation-Action header",
        )),
    }
}

fn generated_naming(headers: &HeaderMap) -> Result<GeneratedNaming, Response> {
    let extension = optional_header(headers, "file-extension", "File-Extension")?
        .map(|extension| crate::store::normalize_allocated_extension(&extension))
        .transpose()
        .map_err(IntoResponse::into_response)?;
    Ok(GeneratedNaming {
        prefix: optional_header(headers, "file-prefix", "File-Prefix")?.unwrap_or_default(),
        suffix: optional_header(headers, "file-suffix", "File-Suffix")?.unwrap_or_default(),
        extension,
    })
}

fn parse_alias_batch(bytes: &[u8]) -> Result<contract::AliasBatchRequest, Response> {
    let batch = serde_json::from_slice::<contract::AliasBatchRequest>(bytes)
        .map_err(|_| plain(StatusCode::BAD_REQUEST, "error: invalid alias batch JSON"))?;
    if batch.aliases.is_empty() {
        return Err(plain(
            StatusCode::BAD_REQUEST,
            "error: alias batch must contain 1-4096 aliases",
        ));
    }
    if batch.aliases.len() > MAX_ALIAS_BATCH_ENTRIES {
        return Err(plain(
            StatusCode::PAYLOAD_TOO_LARGE,
            "error: alias batch exceeds the configured entry limit",
        ));
    }
    if batch
        .aliases
        .iter()
        .any(|alias| alias.target.len() > crate::upload::MAX_ALIAS_TARGET_BYTES)
    {
        return Err(plain(
            StatusCode::PAYLOAD_TOO_LARGE,
            "error: alias batch target exceeds the configured limit",
        ));
    }
    Ok(batch)
}

fn media_type(headers: &HeaderMap) -> Result<String, Response> {
    optional_header(headers, header::CONTENT_TYPE.as_str(), "Content-Type")
        .map(|value| value.unwrap_or_else(|| "application/octet-stream".to_string()))
}

fn allocation_expiry(
    headers: &HeaderMap,
    defaults: crate::expiry::DecayPolicy,
    absent: FileExpiry,
) -> Result<FileExpiry, Response> {
    if !headers.contains_key("expiry-mode") && !has_expiry_parameters(headers) {
        return Ok(absent);
    }
    match expiry_policy_from(headers, defaults) {
        Ok(ExpiryRequest::Never) => Ok(FileExpiry::Clear),
        Ok(ExpiryRequest::Policy(policy)) => Ok(FileExpiry::Policy(policy)),
        Ok(ExpiryRequest::Default) => unreachable!("expiry headers were present"),
        Err(error) => Err(StoreError::Expiry(error).into_response()),
    }
}

fn idempotency(headers: &HeaderMap) -> Result<Idempotency, Response> {
    let key = match headers.get("idempotency-key") {
        Some(value) => value
            .to_str()
            .map_err(|_| {
                plain(
                    StatusCode::BAD_REQUEST,
                    "error: invalid Idempotency-Key header",
                )
            })?
            .to_string(),
        None => generated_idempotency_key().map_err(IntoResponse::into_response)?,
    };
    if key.is_empty()
        || key.len() > 256
        || !key
            .as_bytes()
            .iter()
            .all(|byte| (0x21..=0x7e).contains(byte))
    {
        return Err(plain(
            StatusCode::BAD_REQUEST,
            "error: idempotency key must be 1-256 visible ASCII characters",
        ));
    }
    Ok(Idempotency { key })
}

fn generated_idempotency_key() -> Result<String, StoreError> {
    let mut random = [0_u8; 32];
    getrandom::fill(&mut random)?;
    let mut encoded = String::with_capacity(64);
    for byte in random {
        write!(encoded, "{byte:02x}").expect("writing to String cannot fail");
    }
    Ok(encoded)
}

fn content_match(headers: &HeaderMap) -> Result<String, Response> {
    let value = required_text_header(headers, "if-content-match", "If-Content-Match")?;
    let value = value.trim();
    let value = value
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .unwrap_or(value);
    let value = value.strip_prefix("blake3:").unwrap_or(value);
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(plain(
            StatusCode::BAD_REQUEST,
            "error: If-Content-Match must contain one raw Blake3 hash",
        ));
    }
    Ok(value.to_string())
}

fn required_text_header(
    headers: &HeaderMap,
    name: &str,
    display_name: &'static str,
) -> Result<String, Response> {
    optional_header(headers, name, display_name)?.ok_or_else(|| {
        plain(
            StatusCode::BAD_REQUEST,
            format!("error: {display_name} header is required"),
        )
    })
}

fn optional_header(
    headers: &HeaderMap,
    name: &str,
    display_name: &'static str,
) -> Result<Option<String>, Response> {
    headers
        .get(name)
        .map(|value| {
            value.to_str().map(str::to_string).map_err(|_| {
                plain(
                    StatusCode::BAD_REQUEST,
                    format!("error: invalid {display_name} header"),
                )
            })
        })
        .transpose()
}

async fn spool_content(
    app: &App,
    headers: &HeaderMap,
    body: Body,
    limit: u64,
) -> Result<TemporaryUpload, Response> {
    if content_length(headers).is_some_and(|size| size > limit) {
        return Err(StoreError::Upload(crate::upload::UploadError::FileTooLarge).into_response());
    }
    let temporary = TemporaryUpload {
        path: app.store.upload_path(),
    };
    let result = async {
        let mut file = tokio::fs::File::create(&temporary.path).await?;
        let mut stream = body.into_data_stream();
        let mut size = 0_u64;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|error| io::Error::other(error.to_string()))?;
            size = size
                .checked_add(u64::try_from(chunk.len()).expect("chunk length fits in u64"))
                .ok_or_else(|| io::Error::other("request body size overflow"))?;
            if size > limit {
                return Err(StoreError::Upload(crate::upload::UploadError::FileTooLarge));
            }
            file.write_all(&chunk).await?;
        }
        file.sync_all().await?;
        Ok::<(), StoreError>(())
    }
    .await;
    result.map_err(IntoResponse::into_response)?;
    Ok(temporary)
}

async fn require_empty_body(body: Body) -> Result<(), Response> {
    match to_bytes(body, 1).await {
        Ok(bytes) if bytes.is_empty() => Ok(()),
        Ok(_) | Err(_) => Err(plain(
            StatusCode::BAD_REQUEST,
            "error: this allocation action requires an empty body",
        )),
    }
}

fn content_length(headers: &HeaderMap) -> Option<u64> {
    headers
        .get(header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
}

fn content_type_is(headers: &HeaderMap, expected: &str) -> bool {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .is_some_and(|value| value.trim().eq_ignore_ascii_case(expected))
}

fn infer_extension(media_type: &str) -> Option<&'static str> {
    match media_type.split(';').next().unwrap_or(media_type).trim() {
        "application/json" => Some("json"),
        "text/plain" => Some("txt"),
        "text/html" => Some("html"),
        "text/css" => Some("css"),
        "text/javascript" | "application/javascript" => Some("js"),
        "image/png" => Some("png"),
        "image/jpeg" => Some("jpg"),
        "image/gif" => Some("gif"),
        "image/webp" => Some("webp"),
        "image/svg+xml" => Some("svg"),
        "application/pdf" => Some("pdf"),
        "application/wasm" => Some("wasm"),
        _ => None,
    }
}

fn prefixed_hash(hash: &str) -> String {
    format!("blake3:{hash}")
}

fn inventory_alias(alias: AliasEntry) -> contract::InventoryAlias {
    contract::InventoryAlias {
        path: alias.path,
        target: alias.canonical_target,
        target_kind: alias.resolved_kind.map(alias_target_kind),
        dangling: alias.resolved_kind.is_none(),
        resolved_hash: alias.resolved_hash.map(|hash| prefixed_hash(&hash)),
        size: alias.resolved_size,
    }
}

fn alias_receipt(alias: AliasEntry, mutation: contract::MutationReceipt) -> contract::AliasReceipt {
    contract::AliasReceipt {
        path: alias.path,
        target: alias.canonical_target,
        target_kind: alias.resolved_kind.map(alias_target_kind),
        dangling: alias.resolved_kind.is_none(),
        resolved_hash: alias.resolved_hash.map(|hash| prefixed_hash(&hash)),
        size: alias.resolved_size,
        mutation,
    }
}

const fn alias_target_kind(kind: AliasResolvedKind) -> contract::AliasTargetKind {
    match kind {
        AliasResolvedKind::File => contract::AliasTargetKind::File,
        AliasResolvedKind::Directory => contract::AliasTargetKind::Directory,
    }
}

fn mutation_receipt(
    mutation: &MutationResult,
    idempotency_key: &str,
    location: String,
) -> contract::MutationReceipt {
    contract::MutationReceipt {
        changed: mutation.changed,
        replayed: mutation.replayed,
        idempotency_key: idempotency_key.to_string(),
        location,
        etag: mutation.tree_hash.clone(),
        content_revision: mutation.revision,
        sanitized_management_tokens: u64::try_from(mutation.sanitized.management)
            .expect("sanitized token count fits in u64"),
        sanitized_creator_claims: u64::try_from(mutation.sanitized.claim)
            .expect("sanitized token count fits in u64"),
        undo: mutation.undo.as_ref().map(|undo| contract::UndoReceipt {
            token: undo.token.clone(),
            expires_at: undo.expires_at.clone(),
        }),
    }
}

async fn allocated_mutation_receipt(
    app: &App,
    name: &str,
    allocated: &AllocatedFile,
    idempotency_key: &str,
    location: String,
) -> Result<contract::MutationReceipt, StoreError> {
    if let Some(mutation) = &allocated.mutation {
        return Ok(mutation_receipt(mutation, idempotency_key, location));
    }
    let snapshot = app
        .run_store({
            let name = name.to_string();
            move |store| store.site_inventory(&name)
        })
        .await?;
    Ok(contract::MutationReceipt {
        changed: allocated.changed,
        replayed: allocated.replayed,
        idempotency_key: idempotency_key.to_string(),
        location,
        etag: snapshot.tree_hash,
        content_revision: snapshot.content_revision,
        sanitized_management_tokens: 0,
        sanitized_creator_claims: 0,
        undo: None,
    })
}

fn insert_mutation_headers(headers: &mut HeaderMap, mutation: &MutationResult, location: &str) {
    headers.insert(
        header::LOCATION,
        HeaderValue::from_str(location).expect("valid location"),
    );
    headers.insert(
        header::ETAG,
        HeaderValue::from_str(&format!("\"{}\"", mutation.tree_hash)).expect("valid ETag"),
    );
    headers.insert(
        "content-revision",
        HeaderValue::from_str(&mutation.revision.to_string()).expect("valid revision"),
    );
    if mutation.replayed {
        headers.insert("idempotency-replayed", HeaderValue::from_static("true"));
    }
    if let Some(undo) = &mutation.undo {
        insert_undo_headers(headers, undo);
    }
    insert_sanitized_headers(headers, mutation.sanitized);
}

fn insert_snapshot_values(headers: &mut HeaderMap, location: &str, tree_hash: &str, revision: u64) {
    headers.insert(
        header::LOCATION,
        HeaderValue::from_str(location).expect("valid location"),
    );
    headers.insert(
        header::ETAG,
        HeaderValue::from_str(&format!("\"{tree_hash}\"")).expect("valid ETag"),
    );
    headers.insert(
        "content-revision",
        HeaderValue::from_str(&revision.to_string()).expect("valid revision"),
    );
}

fn insert_replay_header(headers: &mut HeaderMap, replayed: bool) {
    if replayed {
        headers.insert("idempotency-replayed", HeaderValue::from_static("true"));
    }
}

async fn insert_allocated_headers(
    app: &App,
    name: &str,
    allocated: &AllocatedFile,
    location: &str,
    headers: &mut HeaderMap,
) -> Result<(), StoreError> {
    if let Some(mutation) = &allocated.mutation {
        insert_mutation_headers(headers, mutation, location);
        return Ok(());
    }
    headers.insert(
        header::LOCATION,
        HeaderValue::from_str(location).expect("valid location"),
    );
    if allocated.replayed {
        headers.insert("idempotency-replayed", HeaderValue::from_static("true"));
    }
    let snapshot = app
        .run_store({
            let name = name.to_string();
            move |store| store.site_inventory(&name)
        })
        .await?;
    headers.insert(
        header::ETAG,
        HeaderValue::from_str(&format!("\"{}\"", snapshot.tree_hash)).expect("valid ETag"),
    );
    headers.insert(
        "content-revision",
        HeaderValue::from_str(&snapshot.content_revision.to_string()).expect("valid revision"),
    );
    Ok(())
}

async fn mutation_error(app: &App, name: &str, error: StoreError) -> Response {
    if let StoreError::StaleContentHash(current_hash) = error {
        let revision = app
            .run_store({
                let name = name.to_string();
                move |store| store.site_inventory(&name)
            })
            .await
            .map_or(0, |snapshot| snapshot.content_revision);
        let mut response = plain(
            StatusCode::PRECONDITION_FAILED,
            format!("error: file content hash is stale; current hash is {current_hash}"),
        );
        response.headers_mut().insert(
            header::ETAG,
            HeaderValue::from_str(&format!("\"{current_hash}\"")).expect("valid content ETag"),
        );
        response.headers_mut().insert(
            "content-revision",
            HeaderValue::from_str(&revision.to_string()).expect("valid revision"),
        );
        response
    } else {
        error.into_response()
    }
}

fn splice_error(error: &ProtocolError) -> Response {
    let status = match error {
        ProtocolError::HeaderLimit | ProtocolError::FrameLimit | ProtocolError::PayloadTooLarge => {
            StatusCode::PAYLOAD_TOO_LARGE
        }
        ProtocolError::Io(_) => StatusCode::INTERNAL_SERVER_ERROR,
        ProtocolError::MalformedDescriptor
        | ProtocolError::MissingDescriptors
        | ProtocolError::MalformedFrame
        | ProtocolError::PayloadLength => StatusCode::BAD_REQUEST,
    };
    plain(status, error.to_string())
}

fn site_location(app: &App, name: &str) -> String {
    resource_location(app, name, "")
}

fn resource_location(app: &App, name: &str, path: &str) -> String {
    let mut location = format!("{}/{}", app.public_url, name);
    if path.is_empty() {
        location.push('/');
        return location;
    }
    for segment in path.split('/') {
        location.push('/');
        encode_path_segment(&mut location, segment);
    }
    location
}

fn encode_path_segment(output: &mut String, segment: &str) {
    for byte in segment.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            output.push(char::from(byte));
        } else {
            write!(output, "%{byte:02X}").expect("writing to String cannot fail");
        }
    }
}

#[allow(dead_code)]
fn _store_type_anchor(_: &Store, _: PathBuf) {}
