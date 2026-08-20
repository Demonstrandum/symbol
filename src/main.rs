macro_rules! static_asset {
    ($name:literal) => {
        include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/static/", $name))
    };
}

mod blob_store;
mod browse;
mod expiry;
mod http_cache;
mod name;
mod page;
mod pathutil;
mod sanitize;
mod secrets;
mod store;
mod upload;

use std::collections::HashMap;
use std::io::{self, SeekFrom};
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::extract::{ConnectInfo, Path, State};
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, Request, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::get;
use axum::{Json, Router};
use clap::{Parser, Subcommand};
use expiry::{DecayPolicy, ExpiryMode, ExpiryPolicy};
use futures_util::StreamExt as _;
use secrets::{ClaimToken, ManagementToken};
use store::{
    ArchiveFormat, CreationSecurity, CreatorIdentity, Idempotency, PublishOptions, Store,
    StoreError,
};
use tokio::io::{AsyncReadExt as _, AsyncSeekExt as _, AsyncWriteExt as _};
use tokio::net::TcpListener;
use tokio::sync::Semaphore;
use tokio_util::io::ReaderStream;
use tower_http::trace::TraceLayer;

const MAX_ARCHIVE_UPLOAD: usize = 50 * 1024 * 1024;
const DEFAULT_MAX_FILE_SIZE: u64 = 4 * 1024 * 1024 * 1024;
const STREAM_THRESHOLD: u64 = 1024 * 1024;
const INSTALL_SH: &str = static_asset!("install.sh");
const SYMBOL_SH: &str = static_asset!("symbol.sh");

#[derive(Parser)]
#[command(name = "symbol", about = "Tiny static-site hosting for the tailnet")]
struct Args {
    #[arg(long, default_value = "127.0.0.1:4340", env = "SYMBOL_BIND")]
    bind: String,
    #[arg(long, default_value = "/var/lib/symbol", env = "SYMBOL_ROOT")]
    root: PathBuf,
    #[arg(
        long,
        default_value_t = DEFAULT_MAX_FILE_SIZE,
        env = "SYMBOL_MAX_FILE_SIZE"
    )]
    max_file_size: u64,
    #[arg(long, env = "SYMBOL_PUBLIC_URL")]
    public_url: Option<String>,
    #[arg(long, env = "SYMBOL_ALLOW_DEV_ORIGIN", default_value_t = false)]
    allow_dev_origin: bool,
    #[arg(long, env = "SYMBOL_EXPIRY_MIN_AGE", default_value = "30d")]
    expiry_min_age: String,
    #[arg(long, env = "SYMBOL_EXPIRY_MAX_AGE", default_value = "365d")]
    expiry_max_age: String,
    #[arg(long, env = "SYMBOL_EXPIRY_MAX_SIZE", default_value = "512MiB")]
    expiry_max_size: String,
    #[arg(long, env = "SYMBOL_EXPIRY_POWER", default_value_t = 3.0)]
    expiry_power: f64,
    #[arg(long, env = "SYMBOL_TRUSTED_PROXY_PRINCIPAL_HEADER")]
    trusted_proxy_principal_header: Option<HeaderName>,
    #[arg(long, env = "SYMBOL_TRUSTED_PROXY", value_delimiter = ',')]
    trusted_proxy: Vec<IpAddr>,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    Admin {
        #[command(subcommand)]
        action: AdminAction,
    },
}

#[derive(Subcommand)]
enum AdminAction {
    Claim {
        name: String,
    },
    Rotate {
        name: String,
        #[arg(long)]
        token: String,
    },
}

#[derive(Clone)]
struct App {
    store: Store,
    store_tasks: Arc<Semaphore>,
    hashes: Arc<Mutex<HashMap<String, String>>>,
    max_file_size: u64,
    public_url: Arc<str>,
    identity_provider: IdentityProvider,
}

#[derive(Clone)]
enum IdentityProvider {
    // Tailscale can be added as another startup-selected provider once a stable,
    // authenticated local-daemon API contract is chosen; caller headers are never trusted.
    Receipt,
    TrustedProxy {
        principal_header: HeaderName,
        peers: Arc<[IpAddr]>,
    },
}

const INTERNAL_CREATOR_HEADER: &str = "x-symbol-internal-creator-principal";

struct TemporaryUpload {
    path: PathBuf,
}

#[derive(Clone, Copy)]
enum UploadLimitKind {
    Archive,
    File,
}

struct PublishRequestOptions {
    expected_tree_hash: Option<String>,
    idempotency: Option<Idempotency>,
    creation: CreationSecurity,
    authorization: Option<ManagementToken>,
}

struct CreationRequest {
    security: CreationSecurity,
    management_token: Option<ManagementToken>,
    claim_token: Option<ClaimToken>,
    managed: bool,
}

#[derive(Clone, Copy)]
enum ManagementAction {
    Claim,
    Status,
    Rotate,
    Release,
}

impl UploadLimitKind {
    const fn error(self) -> upload::UploadError {
        match self {
            Self::Archive => upload::UploadError::ArchiveTooLarge,
            Self::File => upload::UploadError::FileTooLarge,
        }
    }
}

impl Drop for TemporaryUpload {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

impl App {
    #[cfg(test)]
    fn new(store: Store) -> Self {
        Self::with_options(
            store,
            DEFAULT_MAX_FILE_SIZE,
            "http://symbol".into(),
            IdentityProvider::Receipt,
        )
    }

    #[cfg(test)]
    fn with_max_file_size(store: Store, max_file_size: u64) -> Self {
        Self::with_options(
            store,
            max_file_size,
            "http://symbol".into(),
            IdentityProvider::Receipt,
        )
    }

    fn with_options(
        store: Store,
        max_file_size: u64,
        public_url: String,
        identity_provider: IdentityProvider,
    ) -> Self {
        Self {
            store_tasks: Arc::new(Semaphore::new(store.blocking_capacity())),
            store,
            hashes: Arc::new(Mutex::new(HashMap::new())),
            max_file_size,
            public_url: public_url.into(),
            identity_provider,
        }
    }

    async fn run_store<T, F>(&self, work: F) -> Result<T, StoreError>
    where
        T: Send + 'static,
        F: FnOnce(Store) -> Result<T, StoreError> + Send + 'static,
    {
        let permit = Arc::clone(&self.store_tasks)
            .acquire_owned()
            .await
            .expect("store semaphore stays open");
        let store = self.store.clone();
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            work(store)
        })
        .await
        .expect("store task panicked")
    }
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let args = Args::parse();
    let is_admin = args.command.is_some();
    let public_url = args.public_url.unwrap_or_else(|| {
        assert!(
            args.allow_dev_origin || is_admin,
            "SYMBOL_PUBLIC_URL is required (or set SYMBOL_ALLOW_DEV_ORIGIN=true for development)"
        );
        "http://symbol".to_string()
    });
    validate_public_url(&public_url).expect("valid SYMBOL_PUBLIC_URL");
    let expiry_defaults = DecayPolicy {
        min_age_seconds: expiry::parse_duration_seconds(&args.expiry_min_age)
            .expect("valid SYMBOL_EXPIRY_MIN_AGE"),
        max_age_seconds: expiry::parse_duration_seconds(&args.expiry_max_age)
            .expect("valid SYMBOL_EXPIRY_MAX_AGE"),
        max_size_bytes: expiry::parse_size_bytes(&args.expiry_max_size)
            .expect("valid SYMBOL_EXPIRY_MAX_SIZE"),
        power: args.expiry_power,
    };
    let store = Store::with_expiry_defaults(args.root, public_url.clone(), expiry_defaults)
        .expect("create data directory");
    if let Some(Command::Admin { action }) = args.command {
        let (name, token, verb) = match action {
            AdminAction::Claim { name } => {
                let token = store
                    .operator_claim(&name)
                    .expect("operator management claim");
                (name, token, "claimed")
            }
            AdminAction::Rotate { name, token } => {
                let current =
                    ManagementToken::parse(&token).expect("valid current management token");
                let replacement = store
                    .operator_rotate(&name, &current)
                    .expect("operator management rotation");
                (name, replacement, "rotated")
            }
        };
        println!("operator-{verb} {name}");
        println!("management token (shown once):");
        println!("  {}", token.encode());
        return;
    }
    let identity_provider = match (
        args.trusted_proxy_principal_header,
        args.trusted_proxy.is_empty(),
    ) {
        (None, true) => IdentityProvider::Receipt,
        (Some(principal_header), false) => IdentityProvider::TrustedProxy {
            principal_header,
            peers: args.trusted_proxy.into(),
        },
        _ => panic!(
            "SYMBOL_TRUSTED_PROXY_PRINCIPAL_HEADER and SYMBOL_TRUSTED_PROXY must be configured together"
        ),
    };
    let state = App::with_options(store, args.max_file_size, public_url, identity_provider);
    tokio::spawn(expiry_worker(state.clone()));
    let app = router(state);
    let listener = TcpListener::bind(&args.bind)
        .await
        .unwrap_or_else(|err| panic!("bind {}: {err}", args.bind));
    tracing::info!("listening on {}", args.bind);
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown())
    .await
    .expect("server");
}

fn router(app: App) -> Router {
    let identity_provider = app.identity_provider.clone();
    Router::new()
        .route("/", get(docs).put(put_site_unnamed))
        .route("/HASH", get(docs_hash))
        .route("/STATS", get(stats))
        .route("/STATS/", get(stats))
        .route("/install.sh", get(install_sh))
        .route("/install.sh/HASH", get(install_sh_hash))
        .route("/symbol.sh", get(symbol_sh))
        .route("/symbol.sh/HASH", get(symbol_sh_hash))
        .route("/FILES", get(list_sites))
        .route("/FILES/", get(list_sites))
        .route("/{name}/FILES", get(browse_root))
        .route("/{name}/FILES/", get(browse_root))
        .route("/{name}/FILES/{*path}", get(browse_path))
        .route("/{name}/UNDO", get(undo_stack))
        .route("/{name}/UNDO/", get(undo_stack))
        .route("/{name}/EXPIRES", get(expiry_site_report))
        .route("/{name}/EXPIRES/", get(expiry_site_report))
        .route(
            "/{name}/",
            get(serve_index)
                .put(put_site)
                .delete(delete_site)
                .fallback(lifecycle_method),
        )
        .route("/.blob/{name}/{hash}", get(serve_immutable_blob))
        .route(
            "/{name}/{*path}",
            get(serve_path)
                .put(put_file)
                .delete(delete_file)
                .fallback(content_method),
        )
        .route(
            "/{name}",
            get(redirect_site)
                .put(put_site)
                .delete(delete_site)
                .fallback(lifecycle_method),
        )
        .layer(TraceLayer::new_for_http())
        .layer(middleware::from_fn_with_state(
            identity_provider,
            resolve_creator,
        ))
        .with_state(app)
}

async fn resolve_creator(
    State(provider): State<IdentityProvider>,
    mut request: Request<Body>,
    next: Next,
) -> Response {
    request.headers_mut().remove(INTERNAL_CREATOR_HEADER);
    if let IdentityProvider::TrustedProxy {
        principal_header,
        peers,
    } = provider
        && let Some(peer) = request
            .extensions()
            .get::<ConnectInfo<SocketAddr>>()
            .map(|peer| peer.0.ip())
        && peers.contains(&peer)
        && let Some(principal) = request.headers().get(&principal_header).cloned()
    {
        request
            .headers_mut()
            .insert(INTERNAL_CREATOR_HEADER, principal);
    }
    next.run(request).await
}

async fn shutdown() {
    let ctrl_c = tokio::signal::ctrl_c();
    #[cfg(unix)]
    {
        let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("sigterm");
        tokio::select! {
            _ = ctrl_c => {}
            _ = term.recv() => {}
        }
    }
    #[cfg(not(unix))]
    ctrl_c.await.ok();
}

fn validate_public_url(url: &str) -> Result<(), &'static str> {
    if !(url.starts_with("http://") || url.starts_with("https://"))
        || url.ends_with('/')
        || url
            .split_once("://")
            .is_none_or(|(_, rest)| rest.is_empty() || rest.contains('/'))
    {
        return Err("must be an absolute http(s) origin without a trailing slash or path");
    }
    Ok(())
}

fn script_body(template: &str, public_url: &str) -> String {
    template.replace("__HOST__", public_url)
}

fn render_script(template: &str, public_url: &str, headers: &HeaderMap) -> Response {
    let body = script_body(template, public_url);
    http_cache::respond(
        headers,
        http_cache::Representation::new(body, "text/x-shellscript; charset=utf-8"),
    )
}

fn cached_hash(app: &App, key: &str, bytes: &[u8]) -> String {
    let mut cache = app.hashes.lock().unwrap();
    if let Some(hash) = cache.get(key) {
        return hash.clone();
    }
    let hash = blake3::hash(bytes).to_hex().to_string();
    cache.insert(key.to_string(), hash.clone());
    hash
}

fn hash_body(app: &App, key: &str, bytes: &[u8]) -> Response {
    plain(StatusCode::OK, cached_hash(app, key, bytes))
}

async fn docs_hash(State(app): State<App>, _headers: HeaderMap) -> Response {
    let host = &app.public_url;
    let body = page::render_plain(host);
    hash_body(&app, &format!("docs:{host}"), body.as_bytes())
}

async fn install_sh_hash(State(app): State<App>) -> Response {
    hash_body(&app, "install.sh", INSTALL_SH.as_bytes())
}

async fn symbol_sh_hash(State(app): State<App>) -> Response {
    hash_body(&app, "symbol.sh", SYMBOL_SH.as_bytes())
}

async fn stats(State(app): State<App>) -> Response {
    match app.run_store(|store| store.stats()).await {
        Ok(s) => Json(s).into_response(),
        Err(err) => err.into_response(),
    }
}

fn strip_hash_path(path: &str) -> Option<&str> {
    let path = path.trim_end_matches('/');
    if path == "HASH" {
        return Some("");
    }
    path.strip_suffix("/HASH")
}

async fn send_hash(app: &App, name: &str, rel: &str) -> Response {
    let name = name.to_string();
    let rel = rel.to_string();
    let lookup = app
        .run_store(move |store| lookup_hash(&store, &name, &rel))
        .await;
    match lookup {
        Ok(hash) => hash.map_or_else(
            || StoreError::NotFound.into_response(),
            |hash| plain(StatusCode::OK, hash),
        ),
        Err(err) => err.into_response(),
    }
}

fn lookup_hash(store: &Store, name: &str, rel: &str) -> Result<Option<String>, StoreError> {
    if rel.is_empty() {
        return Ok(["index.html", "index.htm"].iter().find_map(|index| {
            store
                .child_blob(name, "", index)
                .ok()
                .and_then(|node| match node {
                    store::Node::File { hash, .. } => Some(hash),
                    store::Node::Dir => None,
                })
        }));
    }
    match store.lookup(name, rel) {
        Ok(store::Node::File { hash, .. }) => Ok(Some(hash)),
        Ok(store::Node::Dir) | Err(StoreError::NotFound) => Ok(None),
        Err(err) => Err(err),
    }
}

async fn docs(State(app): State<App>, headers: HeaderMap) -> Response {
    page::render(&headers, &app.public_url, page::negotiate(&headers))
}

async fn install_sh(State(app): State<App>, headers: HeaderMap) -> Response {
    render_script(INSTALL_SH, &app.public_url, &headers)
}

async fn symbol_sh(State(app): State<App>, headers: HeaderMap) -> Response {
    render_script(SYMBOL_SH, &app.public_url, &headers)
}

async fn list_sites(State(app): State<App>, headers: HeaderMap) -> Response {
    match app.run_store(|store| store.list_sites()).await {
        Ok(names) => browse::sites(&headers, &names),
        Err(err) => err.into_response(),
    }
}

async fn undo_stack(State(app): State<App>, Path(name): Path<String>) -> Response {
    let result = app
        .run_store({
            let name = name.clone();
            move |store| store.undo_stack(&name)
        })
        .await;
    match result {
        Ok(stack) => {
            let mut response = Json(stack).into_response();
            response
                .headers_mut()
                .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
            response
        }
        Err(err) => err.into_response(),
    }
}

async fn expiry_worker(app: App) {
    loop {
        expiry_worker_iteration(&app).await;
    }
}

#[allow(clippy::cognitive_complexity)]
async fn expiry_worker_iteration(app: &App) {
    let delay = match app.run_store(|store| store.next_expiry_delay()).await {
        Ok(delay) => delay,
        Err(err) => {
            tracing::warn!(%err, "failed to determine next expiry deadline");
            std::time::Duration::from_secs(60)
        }
    };
    tokio::time::sleep(delay).await;
    match app.run_store(|store| store.sweep_expired()).await {
        Ok(0) => {}
        Ok(expired) => tracing::info!(expired, "expired hosted targets"),
        Err(err) => tracing::warn!(%err, "expiry sweep failed"),
    }
}

async fn expiry_site_report(State(app): State<App>, Path(name): Path<String>) -> Response {
    expiry_report_response(&app, name, String::new()).await
}

async fn expiry_path_report(app: &App, name: String, path: String) -> Response {
    expiry_report_response(app, name, path).await
}

async fn expiry_report_response(app: &App, name: String, path: String) -> Response {
    let result = app
        .run_store(move |store| store.expiry_report(&name, &path))
        .await;
    match result {
        Ok(report) => {
            let mut response = Json(report).into_response();
            response
                .headers_mut()
                .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
            response
        }
        Err(err) => err.into_response(),
    }
}

async fn lifecycle_method(
    State(app): State<App>,
    Path(name): Path<String>,
    method: Method,
    headers: HeaderMap,
) -> Response {
    match method.as_str() {
        "UNDO" => undo_site(&app, &name, &headers).await,
        "COPY" => copy_site(&app, &name, &headers).await,
        "MOVE" => move_site(&app, &name, &headers).await,
        "EXPIRE" => expire_target(&app, &name, "", &headers).await,
        "MANAGE" => manage_site(&app, &name, &headers).await,
        _ => plain(StatusCode::METHOD_NOT_ALLOWED, "error: method not allowed"),
    }
}

async fn manage_site(app: &App, name: &str, headers: &HeaderMap) -> Response {
    let action = match headers
        .get("management-action")
        .and_then(|value| value.to_str().ok())
    {
        Some("claim") => ManagementAction::Claim,
        Some("status") => ManagementAction::Status,
        Some("rotate") => ManagementAction::Rotate,
        Some("release") => ManagementAction::Release,
        Some(_) => {
            return plain(
                StatusCode::BAD_REQUEST,
                "error: invalid Management-Action header",
            );
        }
        None => {
            return plain(
                StatusCode::BAD_REQUEST,
                "error: Management-Action header is required",
            );
        }
    };
    let bearer = match management_bearer(headers) {
        Ok(token) => token,
        Err(err) => return err.into_response(),
    };
    let claim = match creator_claim(headers) {
        Ok(claim) => claim,
        Err(message) => return plain(StatusCode::BAD_REQUEST, message),
    };
    let creator = creator_identity(headers);
    let idempotency = idempotency_from(headers);
    let name_owned = name.to_string();
    let result = app
        .run_store(move |store| match action {
            ManagementAction::Claim => {
                store.claim_management(&name_owned, creator, claim.as_ref(), idempotency.as_ref())
            }
            ManagementAction::Rotate => store.rotate_management(
                &name_owned,
                bearer.as_ref(),
                creator,
                claim.as_ref(),
                idempotency.as_ref(),
            ),
            ManagementAction::Status => {
                store
                    .management_status(&name_owned)
                    .map(|status| store::ManagementMutation {
                        status,
                        token: None,
                        replayed: false,
                    })
            }
            ManagementAction::Release => store
                .release_management(&name_owned, bearer.as_ref())
                .map(|status| store::ManagementMutation {
                    status,
                    token: None,
                    replayed: false,
                }),
        })
        .await;
    match result {
        Ok(mutation) => {
            let mut response = Json(mutation.status).into_response();
            response
                .headers_mut()
                .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
            if let Some(token) = mutation.token {
                response.headers_mut().insert(
                    "management-token",
                    HeaderValue::from_str(&token.encode()).expect("token is a valid header"),
                );
            }
            if mutation.replayed {
                response
                    .headers_mut()
                    .insert("idempotency-replayed", HeaderValue::from_static("true"));
            }
            response
        }
        Err(err) => err.into_response(),
    }
}

async fn content_method(
    State(app): State<App>,
    Path((name, path)): Path<(String, String)>,
    method: Method,
    headers: HeaderMap,
) -> Response {
    match method.as_str() {
        "EXPIRE" => expire_target(&app, &name, path.trim_end_matches('/'), &headers).await,
        _ => plain(StatusCode::METHOD_NOT_ALLOWED, "error: method not allowed"),
    }
}

async fn expire_target(app: &App, name: &str, path: &str, headers: &HeaderMap) -> Response {
    let authorization = match management_bearer(headers) {
        Ok(token) => token,
        Err(err) => return err.into_response(),
    };
    let auth = authorization.clone();
    let auth_name = name.to_string();
    if let Err(err) = app
        .run_store(move |store| store.authorize_mutation(&auth_name, auth.as_ref()))
        .await
    {
        return err.into_response();
    }
    let policy = match expiry_policy_from(headers, app.store.expiry_defaults()) {
        Ok(policy) => policy,
        Err(err) => return StoreError::Expiry(err).into_response(),
    };
    let name = name.to_string();
    let path = path.to_string();
    let result = app
        .run_store(move |store| match policy {
            ExpiryRequest::Default => {
                store.set_default_expiry_secured(&name, &path, authorization.as_ref())
            }
            ExpiryRequest::Never => {
                store.set_expiry_secured(&name, &path, None, authorization.as_ref())
            }
            ExpiryRequest::Policy(policy) => {
                store.set_expiry_secured(&name, &path, Some(policy), authorization.as_ref())
            }
        })
        .await;
    match result {
        Ok(mutation) => {
            let mut response = Json(mutation.report.clone()).into_response();
            let response_headers = response.headers_mut();
            response_headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
            if let Some(undo) = &mutation.undo {
                insert_undo_headers(response_headers, undo);
            }
            insert_expiry_headers(response_headers, &mutation.report);
            response
        }
        Err(err) => err.into_response(),
    }
}

#[derive(Clone, Copy)]
enum ExpiryRequest {
    Default,
    Never,
    Policy(ExpiryPolicy),
}

fn expiry_policy_from(
    headers: &HeaderMap,
    defaults: DecayPolicy,
) -> Result<ExpiryRequest, expiry::ExpiryError> {
    let Some(mode) = optional_header(headers, "expiry-mode")? else {
        if has_expiry_parameters(headers) {
            return Err(expiry::ExpiryError::InvalidModeHeader);
        }
        return Ok(ExpiryRequest::Default);
    };
    match mode {
        "never" if !has_expiry_parameters(headers) => Ok(ExpiryRequest::Never),
        "relative" => {
            if headers.contains_key("expiry-at") || has_decay_parameters(headers) {
                return Err(expiry::ExpiryError::InvalidModeHeader);
            }
            let duration = required_header(headers, "expiry-in", "Expiry-In")?;
            Ok(ExpiryRequest::Policy(ExpiryPolicy::Relative {
                duration_seconds: expiry::parse_duration_seconds(duration)?,
            }))
        }
        "absolute" => {
            if headers.contains_key("expiry-in") || has_decay_parameters(headers) {
                return Err(expiry::ExpiryError::InvalidModeHeader);
            }
            let timestamp = required_header(headers, "expiry-at", "Expiry-At")?;
            Ok(ExpiryRequest::Policy(ExpiryPolicy::Absolute {
                deadline_unix_seconds: expiry::parse_rfc3339_timestamp(timestamp)?.unix_timestamp(),
            }))
        }
        "decay" => {
            if headers.contains_key("expiry-in") || headers.contains_key("expiry-at") {
                return Err(expiry::ExpiryError::InvalidModeHeader);
            }
            let min_age_seconds = optional_header(headers, "expiry-min-age")?
                .map(expiry::parse_duration_seconds)
                .transpose()?
                .unwrap_or(defaults.min_age_seconds);
            let max_age_seconds = optional_header(headers, "expiry-max-age")?
                .map(expiry::parse_duration_seconds)
                .transpose()?
                .unwrap_or(defaults.max_age_seconds);
            let max_size_bytes = optional_header(headers, "expiry-max-size")?
                .map(expiry::parse_size_bytes)
                .transpose()?
                .unwrap_or(defaults.max_size_bytes);
            let power = optional_header(headers, "expiry-power")?
                .map(|value| {
                    value
                        .parse::<f64>()
                        .map_err(|_| expiry::ExpiryError::InvalidPower)
                })
                .transpose()?
                .unwrap_or(defaults.power);
            Ok(ExpiryRequest::Policy(ExpiryPolicy::Decay(
                DecayPolicy {
                    min_age_seconds,
                    max_age_seconds,
                    max_size_bytes,
                    power,
                }
                .validate()?,
            )))
        }
        _ => Err(expiry::ExpiryError::InvalidModeHeader),
    }
}

fn has_decay_parameters(headers: &HeaderMap) -> bool {
    [
        "expiry-min-age",
        "expiry-max-age",
        "expiry-max-size",
        "expiry-power",
    ]
    .iter()
    .any(|name| headers.contains_key(*name))
}

fn has_expiry_parameters(headers: &HeaderMap) -> bool {
    headers.contains_key("expiry-in")
        || headers.contains_key("expiry-at")
        || has_decay_parameters(headers)
}

fn optional_header<'a>(
    headers: &'a HeaderMap,
    name: &str,
) -> Result<Option<&'a str>, expiry::ExpiryError> {
    headers
        .get(name)
        .map(|value| {
            value
                .to_str()
                .map_err(|_| expiry::ExpiryError::InvalidDuration)
        })
        .transpose()
}

fn required_header<'a>(
    headers: &'a HeaderMap,
    name: &str,
    display_name: &'static str,
) -> Result<&'a str, expiry::ExpiryError> {
    optional_header(headers, name)?.ok_or(expiry::ExpiryError::MissingHeader(display_name))
}

async fn undo_site(app: &App, name: &str, headers: &HeaderMap) -> Response {
    let authorization = match management_bearer(headers) {
        Ok(token) => token,
        Err(err) => return err.into_response(),
    };
    let auth = authorization.clone();
    let auth_name = name.to_string();
    if let Err(err) = app
        .run_store(move |store| store.authorize_mutation(&auth_name, auth.as_ref()))
        .await
    {
        return err.into_response();
    }
    let guard = headers
        .get("undo-token")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let result = app
        .run_store({
            let name = name.to_string();
            move |store| store.undo_secured(&name, guard.as_deref(), authorization.as_ref())
        })
        .await;
    match result {
        Ok(restored) => plain(
            StatusCode::OK,
            format!("restored {name} to {}", restored.restored_at),
        ),
        Err(err) => err.into_response(),
    }
}

async fn copy_site(app: &App, source: &str, headers: &HeaderMap) -> Response {
    let creation = match creation_request(headers) {
        Ok(creation) => creation,
        Err(response) => return response,
    };
    let destination = match destination_from(headers, false) {
        Ok(destination) => destination,
        Err(message) => return plain(StatusCode::BAD_REQUEST, message),
    };
    let idempotency = if destination.is_none() {
        idempotency_from(headers)
    } else {
        None
    };
    let source = source.to_string();
    let result = app
        .run_store(move |store| {
            store.copy_site_secured(
                &source,
                destination.as_deref(),
                idempotency.as_ref(),
                creation.security,
            )
        })
        .await;
    match result {
        Ok((name, mutation)) => {
            let url = format!("{}/{name}/", app.public_url);
            let mut response = mutation_response(
                StatusCode::CREATED,
                &url,
                format!("ok {name} {url} ({} files)", mutation.files),
                &mutation,
            );
            insert_creation_headers(
                &mut response,
                creation,
                mutation.created && !mutation.replayed,
            );
            response
        }
        Err(err) => err.into_response(),
    }
}

async fn move_site(app: &App, source: &str, headers: &HeaderMap) -> Response {
    let authorization = match management_bearer(headers) {
        Ok(token) => token,
        Err(err) => return err.into_response(),
    };
    let auth = authorization.clone();
    let auth_source = source.to_string();
    if let Err(err) = app
        .run_store(move |store| store.authorize_mutation(&auth_source, auth.as_ref()))
        .await
    {
        return err.into_response();
    }
    let Some(destination) = (match destination_from(headers, true) {
        Ok(destination) => destination,
        Err(message) => return plain(StatusCode::BAD_REQUEST, message),
    }) else {
        unreachable!("required destination was checked");
    };
    let source = source.to_string();
    let old_name = source.clone();
    let result = app
        .run_store(move |store| {
            store.move_site_secured(&source, &destination, authorization.as_ref())
        })
        .await;
    match result {
        Ok((name, mutation)) => {
            let url = format!("{}/{name}/", app.public_url);
            mutation_response(
                StatusCode::OK,
                &url,
                format!("moved {}/{old_name}/ -> {url}", app.public_url),
                &mutation,
            )
        }
        Err(err) => err.into_response(),
    }
}

fn destination_from(headers: &HeaderMap, required: bool) -> Result<Option<String>, &'static str> {
    let Some(value) = headers.get("destination") else {
        return if required {
            Err("error: Destination header is required")
        } else {
            Ok(None)
        };
    };
    let value = value
        .to_str()
        .map_err(|_| "error: invalid Destination header")?;
    let path = if value.starts_with('/') {
        value.to_string()
    } else {
        let uri = value
            .parse::<axum::http::Uri>()
            .map_err(|_| "error: invalid Destination header")?;
        uri.path().to_string()
    };
    let name = path.trim_matches('/');
    if name.contains('/') || name::parse_site_name(name).is_err() {
        return Err("error: Destination must identify one valid site");
    }
    Ok(Some(name.to_string()))
}

fn idempotency_from(headers: &HeaderMap) -> Option<Idempotency> {
    headers
        .get("idempotency-key")
        .and_then(|value| value.to_str().ok())
        .map(|key| Idempotency {
            key: key.to_string(),
        })
}

fn management_bearer(headers: &HeaderMap) -> Result<Option<ManagementToken>, StoreError> {
    let Some(value) = headers.get(header::AUTHORIZATION) else {
        return Ok(None);
    };
    let value = value.to_str().map_err(|_| StoreError::Unauthorized)?;
    let encoded = value
        .strip_prefix("Bearer ")
        .ok_or(StoreError::Unauthorized)?;
    ManagementToken::parse(encoded)
        .map(Some)
        .map_err(|_| StoreError::Unauthorized)
}

fn creator_claim(headers: &HeaderMap) -> Result<Option<ClaimToken>, &'static str> {
    headers
        .get("creator-claim")
        .map(|value| {
            value
                .to_str()
                .map_err(|_| "error: invalid Creator-Claim header")
                .and_then(|value| {
                    ClaimToken::parse(value).map_err(|_| "error: invalid Creator-Claim header")
                })
        })
        .transpose()
}

fn creator_identity(headers: &HeaderMap) -> Option<CreatorIdentity> {
    headers
        .get(INTERNAL_CREATOR_HEADER)
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty())
        .map(CreatorIdentity::trusted_proxy)
}

#[allow(clippy::result_large_err)]
fn creation_request(headers: &HeaderMap) -> Result<CreationRequest, Response> {
    let managed = match headers
        .get("management-action")
        .map(|value| value.to_str())
        .transpose()
    {
        Ok(None) => false,
        Ok(Some("claim")) => true,
        Ok(Some(_)) => {
            return Err(plain(
                StatusCode::BAD_REQUEST,
                "error: Management-Action must be claim on creation",
            ));
        }
        Err(_) => {
            return Err(plain(
                StatusCode::BAD_REQUEST,
                "error: invalid Management-Action header",
            ));
        }
    };
    let creator = creator_identity(headers);
    let supplied_claim =
        creator_claim(headers).map_err(|message| plain(StatusCode::BAD_REQUEST, message))?;
    let claim_token = if creator.is_none() && supplied_claim.is_none() {
        Some(
            ClaimToken::generate()
                .map_err(|_| plain(StatusCode::INTERNAL_SERVER_ERROR, "error: random source"))?,
        )
    } else {
        None
    };
    let claim_hash = supplied_claim
        .as_ref()
        .or(claim_token.as_ref())
        .map(ClaimToken::hash);
    let management_token = if managed {
        Some(
            ManagementToken::generate()
                .map_err(|_| plain(StatusCode::INTERNAL_SERVER_ERROR, "error: random source"))?,
        )
    } else {
        None
    };
    Ok(CreationRequest {
        security: CreationSecurity {
            creator,
            claim_hash,
            management_hash: management_token.as_ref().map(ManagementToken::hash),
        },
        management_token,
        claim_token,
        managed,
    })
}

fn insert_creation_headers(response: &mut Response, creation: CreationRequest, created: bool) {
    if !created {
        return;
    }
    let headers = response.headers_mut();
    if let Some(token) = creation.management_token {
        headers.insert(
            "management-token",
            HeaderValue::from_str(&token.encode()).expect("token is a valid header"),
        );
    }
    if let Some(token) = creation.claim_token {
        headers.insert(
            "creator-claim",
            HeaderValue::from_str(&token.encode()).expect("claim is a valid header"),
        );
    }
    if creation.managed || headers.contains_key("creator-claim") {
        headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }
}

fn insert_sanitized_headers(headers: &mut HeaderMap, counts: sanitize::TokenCounts) {
    if counts.management > 0 {
        headers.insert(
            "sanitized-management-tokens",
            HeaderValue::from_str(&counts.management.to_string()).expect("valid count"),
        );
    }
    if counts.claim > 0 {
        headers.insert(
            "sanitized-creator-claims",
            HeaderValue::from_str(&counts.claim.to_string()).expect("valid count"),
        );
    }
}

async fn put_site_unnamed(State(app): State<App>, headers: HeaderMap, body: Body) -> Response {
    publish(&app, None, &headers, body).await
}

async fn put_site(
    State(app): State<App>,
    Path(name): Path<String>,
    headers: HeaderMap,
    body: Body,
) -> Response {
    publish(&app, Some(name), &headers, body).await
}

#[allow(clippy::too_many_lines)]
async fn publish(app: &App, wanted: Option<String>, headers: &HeaderMap, body: Body) -> Response {
    let authorization = match management_bearer(headers) {
        Ok(token) => token,
        Err(err) => return err.into_response(),
    };
    if let Some(name) = wanted.as_deref()
        && app.store.site_exists(name)
    {
        if headers.contains_key("management-action") {
            return StoreError::AlreadyManaged.into_response();
        }
        let auth = authorization.clone();
        let name_owned = name.to_string();
        if let Err(err) = app
            .run_store(move |store| store.authorize_mutation(&name_owned, auth.as_ref()))
            .await
        {
            return err.into_response();
        }
    }
    let creation = match creation_request(headers) {
        Ok(creation) => creation,
        Err(response) => return response,
    };
    let expected_tree_hash = match if_match_from(headers) {
        Ok(expected) => expected,
        Err(message) => return plain(StatusCode::BAD_REQUEST, message),
    };
    let idempotency = wanted
        .is_none()
        .then(|| idempotency_from(headers))
        .flatten();
    let options = PublishRequestOptions {
        expected_tree_hash,
        idempotency,
        creation: creation.security,
        authorization,
    };
    let filename = filename_from(headers);
    let ctype = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok());
    let unpack = wants_unpack(headers);
    let (limit, limit_kind) = if unpack {
        (
            u64::try_from(MAX_ARCHIVE_UPLOAD).expect("archive limit fits in u64"),
            UploadLimitKind::Archive,
        )
    } else {
        (app.max_file_size, UploadLimitKind::File)
    };
    let temporary = match spool_body(app, headers, body, limit, limit_kind).await {
        Ok(path) => path,
        Err(err) => return err.into_response(),
    };
    let prefix = match read_prefix(&temporary.path).await {
        Ok(prefix) => prefix,
        Err(err) => return StoreError::Io(err).into_response(),
    };
    let kind = upload::sniff(&prefix, ctype, filename.as_deref());
    if !unpack
        && matches!(
            kind,
            upload::Kind::Zip | upload::Kind::Tar | upload::Kind::Gzip
        )
    {
        let archive_path = temporary.path.clone();
        let inspection = app
            .run_store(move |_| {
                upload::reject_secrets_in_opaque_archive(&archive_path, kind)
                    .map_err(StoreError::from)
            })
            .await;
        if let Err(err) = inspection {
            return err.into_response();
        }
    }
    let result = if unpack {
        publish_archive(app, wanted, filename, kind, temporary.path.clone(), options).await
    } else {
        let stored_name = filename
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| kind.default_filename().to_string());
        app.run_store({
            let temporary = temporary.path.clone();
            move |store| {
                store.publish_uploaded_file(
                    wanted.as_deref(),
                    &stored_name,
                    temporary,
                    PublishOptions {
                        expected_tree_hash: options.expected_tree_hash.as_deref(),
                        idempotency: options.idempotency.as_ref(),
                        creation: options.creation,
                        authorization: options.authorization.as_ref(),
                    },
                )
            }
        })
        .await
    };
    match result {
        Ok((name, mutation)) => {
            let url = format!("{}/{name}/", app.public_url);
            let mut response = mutation_response(
                if mutation.created {
                    StatusCode::CREATED
                } else {
                    StatusCode::OK
                },
                &url,
                format!(
                    "ok {name} {url} ({} files, changed: {})",
                    mutation.files, mutation.changed
                ),
                &mutation,
            );
            insert_sanitized_headers(response.headers_mut(), mutation.sanitized);
            insert_creation_headers(
                &mut response,
                creation,
                mutation.created && !mutation.replayed,
            );
            response
        }
        Err(err) => err.into_response(),
    }
}

async fn spool_body(
    app: &App,
    headers: &HeaderMap,
    body: Body,
    limit: u64,
    limit_kind: UploadLimitKind,
) -> Result<TemporaryUpload, StoreError> {
    if headers
        .get(header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .is_some_and(|size| size > limit)
    {
        return Err(limit_kind.error().into());
    }
    let path = app.store.upload_path();
    let temporary = TemporaryUpload { path };
    let result = async {
        let mut file = tokio::fs::File::create(&temporary.path).await?;
        let mut stream = body.into_data_stream();
        let mut size = 0_u64;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|err| io::Error::other(err.to_string()))?;
            file.write_all(&chunk).await?;
            size += u64::try_from(chunk.len()).expect("body chunk size fits in u64");
            if size > limit {
                return Err(StoreError::Upload(limit_kind.error()));
            }
        }
        if size == 0 {
            return Err(StoreError::Upload(upload::UploadError::Empty));
        }
        file.sync_all().await?;
        Ok(())
    }
    .await;
    result?;
    Ok(temporary)
}

async fn read_prefix(path: &std::path::Path) -> io::Result<Vec<u8>> {
    let mut file = tokio::fs::File::open(path).await?;
    let mut prefix = vec![0_u8; 512];
    let read = file.read(&mut prefix).await?;
    prefix.truncate(read);
    Ok(prefix)
}

async fn publish_archive(
    app: &App,
    wanted: Option<String>,
    filename: Option<String>,
    kind: upload::Kind,
    path: PathBuf,
    options: PublishRequestOptions,
) -> Result<(String, store::MutationResult), StoreError> {
    app.run_store(move |store| {
        store.publish_uploaded_archive(
            wanted.as_deref(),
            filename.as_deref(),
            &path,
            kind,
            PublishOptions {
                expected_tree_hash: options.expected_tree_hash.as_deref(),
                idempotency: options.idempotency.as_ref(),
                creation: options.creation,
                authorization: options.authorization.as_ref(),
            },
        )
    })
    .await
}

async fn put_file(
    State(app): State<App>,
    Path((name, path)): Path<(String, String)>,
    headers: HeaderMap,
    body: Body,
) -> Response {
    let authorization = match management_bearer(&headers) {
        Ok(token) => token,
        Err(err) => return err.into_response(),
    };
    let auth = authorization.clone();
    let auth_name = name.clone();
    if let Err(err) = app
        .run_store(move |store| store.authorize_mutation(&auth_name, auth.as_ref()))
        .await
    {
        return err.into_response();
    }
    let creation = match creation_request(&headers) {
        Ok(creation) => creation,
        Err(response) => return response,
    };
    let expected_tree_hash = match if_match_from(&headers) {
        Ok(expected) => expected,
        Err(message) => return plain(StatusCode::BAD_REQUEST, message),
    };
    let temporary = match spool_body(
        &app,
        &headers,
        body,
        app.max_file_size,
        UploadLimitKind::File,
    )
    .await
    {
        Ok(path) => path,
        Err(err) => return err.into_response(),
    };
    let result = app
        .run_store({
            let name = name.clone();
            let path = path.clone();
            let temporary = temporary.path.clone();
            move |store| {
                store.put_uploaded_file_secured(
                    &name,
                    &path,
                    temporary,
                    PublishOptions {
                        expected_tree_hash: expected_tree_hash.as_deref(),
                        idempotency: None,
                        creation: creation.security,
                        authorization: authorization.as_ref(),
                    },
                )
            }
        })
        .await;
    match result {
        Ok(mutation) => {
            let url = format!("{}/{name}/{path}", app.public_url);
            let mut response = mutation_response(
                if mutation.created {
                    StatusCode::CREATED
                } else {
                    StatusCode::OK
                },
                &url,
                format!("ok /{name}/{path} (changed: {})", mutation.changed),
                &mutation,
            );
            insert_sanitized_headers(response.headers_mut(), mutation.sanitized);
            insert_creation_headers(
                &mut response,
                creation,
                mutation.created && !mutation.replayed,
            );
            response
        }
        Err(err) => err.into_response(),
    }
}

async fn delete_site(
    State(app): State<App>,
    Path(name): Path<String>,
    headers: HeaderMap,
) -> Response {
    let authorization = match management_bearer(&headers) {
        Ok(token) => token,
        Err(err) => return err.into_response(),
    };
    let auth_name = archive_download(&name).map_or_else(|| name.clone(), |request| request.name);
    let auth = authorization.clone();
    if let Err(err) = app
        .run_store(move |store| store.authorize_mutation(&auth_name, auth.as_ref()))
        .await
    {
        return err.into_response();
    }
    let request = archive_download(&name).unwrap_or(ArchiveDownload {
        name,
        format: ArchiveFormat::TarGz,
        extension: ".tar.gz",
        content_type: "application/gzip",
    });
    let temporary = TemporaryUpload {
        path: app.store.upload_path(),
    };
    let site_name = request.name.clone();
    let format = request.format;
    let result = app
        .run_store({
            let path = temporary.path.clone();
            move |store| {
                store.pop_site_to_path_secured(&site_name, format, &path, authorization.as_ref())
            }
        })
        .await;
    match result {
        Ok(pop) => {
            let mut response = archive_response(
                temporary,
                pop.size,
                request.content_type,
                format!(
                    "attachment; filename=\"{}{}\"",
                    request.name, request.extension
                ),
            )
            .await;
            insert_undo_headers(response.headers_mut(), &pop.undo);
            response
        }
        Err(err) => err.into_response(),
    }
}

async fn delete_file(
    State(app): State<App>,
    Path((name, path)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    let authorization = match management_bearer(&headers) {
        Ok(token) => token,
        Err(err) => return err.into_response(),
    };
    let auth = authorization.clone();
    let auth_name = name.clone();
    if let Err(err) = app
        .run_store(move |store| store.authorize_mutation(&auth_name, auth.as_ref()))
        .await
    {
        return err.into_response();
    }
    let result = app
        .run_store({
            let name = name.clone();
            let path = path.clone();
            move |store| store.delete_file_secured(&name, &path, authorization.as_ref())
        })
        .await;
    match result {
        Ok(mutation) => {
            let mut response = plain(StatusCode::OK, format!("deleted {name}/{path}"));
            if let Some(undo) = &mutation.undo {
                insert_undo_headers(response.headers_mut(), undo);
            }
            response
        }
        Err(err) => err.into_response(),
    }
}

#[derive(Clone)]
struct ArchiveDownload {
    name: String,
    format: ArchiveFormat,
    extension: &'static str,
    content_type: &'static str,
}

fn archive_download(path: &str) -> Option<ArchiveDownload> {
    [
        (".tar.gz", ArchiveFormat::TarGz, "application/gzip"),
        (".tar", ArchiveFormat::Tar, "application/x-tar"),
        (".zip", ArchiveFormat::Zip, "application/zip"),
    ]
    .into_iter()
    .find_map(|(extension, format, content_type)| {
        path.strip_suffix(extension).map(|name| ArchiveDownload {
            name: name.to_string(),
            format,
            extension,
            content_type,
        })
    })
}

async fn download_site(app: &App, request: ArchiveDownload) -> Response {
    let temporary = TemporaryUpload {
        path: app.store.upload_path(),
    };
    let name = request.name.clone();
    let format = request.format;
    let result = app
        .run_store({
            let path = temporary.path.clone();
            move |store| store.pack_site_to_path(&name, format, &path)
        })
        .await;
    match result {
        Ok(size) => {
            let mut response = archive_response(
                temporary,
                size,
                request.content_type,
                format!(
                    "attachment; filename=\"{}{}\"",
                    request.name, request.extension
                ),
            )
            .await;
            add_target_expiry_headers(app, &request.name, "", response.headers_mut()).await;
            response
        }
        Err(err) => err.into_response(),
    }
}

async fn archive_response(
    temporary: TemporaryUpload,
    size: u64,
    content_type: &'static str,
    disposition: String,
) -> Response {
    let file = match tokio::fs::File::open(&temporary.path).await {
        Ok(file) => file,
        Err(err) => return StoreError::Io(err).into_response(),
    };
    let stream =
        futures_util::stream::try_unfold((file, temporary), |(mut file, temporary)| async move {
            let mut bytes = vec![0_u8; 64 * 1024];
            let read = file.read(&mut bytes).await?;
            if read == 0 {
                return Ok(None);
            }
            bytes.truncate(read);
            Ok::<_, io::Error>(Some((bytes::Bytes::from(bytes), (file, temporary))))
        });
    let mut response = Response::new(Body::from_stream(stream));
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    headers.insert(
        header::CONTENT_LENGTH,
        HeaderValue::from_str(&size.to_string()).expect("valid content length"),
    );
    headers.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&disposition).expect("valid content disposition"),
    );
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    response
}

async fn redirect_site(State(app): State<App>, Path(name): Path<String>) -> Response {
    if let Some(request) = archive_download(&name) {
        return download_site(&app, request).await;
    }
    let exists = app
        .run_store({
            let name = name.clone();
            move |store| Ok(store.site_exists(&name))
        })
        .await;
    match exists {
        Ok(true) => Redirect::temporary(&format!("/{name}/")).into_response(),
        Ok(false) => StoreError::NotFound.into_response(),
        Err(err) => err.into_response(),
    }
}

async fn browse_root(
    State(app): State<App>,
    Path(name): Path<String>,
    headers: HeaderMap,
) -> Response {
    if headers
        .get(header::ACCEPT)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value
                .split(',')
                .any(|part| part.trim() == "application/json")
        })
    {
        let result = app
            .run_store({
                let name = name.clone();
                move |store| store.site_inventory(&name)
            })
            .await;
        return match result {
            Ok(inventory) => {
                let revision = inventory.content_revision;
                let etag = inventory.tree_hash.clone();
                let mut response = Json(inventory).into_response();
                let response_headers = response.headers_mut();
                response_headers.insert(
                    header::ETAG,
                    HeaderValue::from_str(&format!("\"{etag}\"")).expect("valid site ETag"),
                );
                response_headers.insert(
                    "content-revision",
                    HeaderValue::from_str(&revision.to_string()).expect("valid revision"),
                );
                response_headers
                    .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
                response
            }
            Err(err) => err.into_response(),
        };
    }
    browse_dir(&app, &name, "", true, &headers).await
}

async fn browse_path(
    State(app): State<App>,
    Path((name, path)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    let rel = path.trim_end_matches('/');
    let node = app
        .run_store({
            let name = name.clone();
            let rel = rel.to_string();
            move |store| store.lookup(&name, &rel)
        })
        .await;
    match node {
        Ok(store::Node::Dir) => {
            if !path.is_empty() && !path.ends_with('/') {
                return Redirect::temporary(&format!("/{name}/FILES/{path}/")).into_response();
            }
            browse_dir(&app, &name, rel, true, &headers).await
        }
        Ok(store::Node::File { .. }) => {
            Redirect::temporary(&format!("/{name}/{rel}")).into_response()
        }
        Err(err) => err.into_response(),
    }
}

async fn browse_dir(
    app: &App,
    name: &str,
    rel: &str,
    files_view: bool,
    headers: &HeaderMap,
) -> Response {
    let result = app
        .run_store({
            let name = name.to_string();
            let rel = rel.to_string();
            move |store| store.list_dir(&name, &rel)
        })
        .await;
    match result {
        Ok(entries) => browse::listing(headers, name, rel, &entries, files_view),
        Err(err) => err.into_response(),
    }
}

async fn serve_index(
    State(app): State<App>,
    Path(name): Path<String>,
    headers: HeaderMap,
) -> Response {
    serve_from(&app, &name, "", &headers).await
}

async fn serve_path(
    State(app): State<App>,
    Path((name, path)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    let control_path = path.trim_end_matches('/');
    if let Some(rel) = control_path.strip_suffix("/EXPIRES") {
        return expiry_path_report(&app, name, rel.to_string()).await;
    }
    if let Some(rel) = strip_hash_path(&path) {
        return send_hash(&app, &name, rel).await;
    }
    serve_from(&app, &name, &path, &headers).await
}

async fn serve_from(app: &App, name: &str, rel: &str, headers: &HeaderMap) -> Response {
    let node = app
        .run_store({
            let name = name.to_string();
            let rel = rel.to_string();
            move |store| store.lookup(&name, &rel)
        })
        .await;
    match node {
        Ok(store::Node::Dir) => {
            if !rel.is_empty() && !rel.ends_with('/') {
                return Redirect::temporary(&format!("/{name}/{rel}/")).into_response();
            }
            let index = app
                .run_store({
                    let name = name.to_string();
                    let rel = rel.to_string();
                    move |store| find_index(&store, &name, &rel)
                })
                .await;
            match index {
                Ok(Some((logical, hash))) => {
                    return send_expiring_blob(headers, name, &logical, &hash, app).await;
                }
                Ok(None) => {}
                Err(err) => return err.into_response(),
            }
            let mut response = browse_dir(app, name, rel, false, headers).await;
            add_target_expiry_headers(app, name, rel, response.headers_mut()).await;
            response
        }
        Ok(store::Node::File { logical, hash }) => {
            send_expiring_blob(headers, name, &logical, &hash, app).await
        }
        Err(err) => err.into_response(),
    }
}

fn find_index(
    store: &Store,
    name: &str,
    rel: &str,
) -> Result<Option<(String, String)>, StoreError> {
    for index in ["index.html", "index.htm"] {
        match store.child_blob(name, rel, index) {
            Ok(store::Node::File { logical, hash }) => return Ok(Some((logical, hash))),
            Ok(store::Node::Dir) | Err(StoreError::NotFound) => {}
            Err(err) => return Err(err),
        }
    }
    Ok(None)
}

async fn serve_immutable_blob(
    State(app): State<App>,
    Path((name, hash)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    let referenced = app
        .run_store({
            let name = name.clone();
            let hash = hash.clone();
            move |store| store.site_references_blob(&name, &hash)
        })
        .await;
    match referenced {
        Ok(true) => {}
        Ok(false) => return StoreError::NotFound.into_response(),
        Err(err) => return err.into_response(),
    }
    send_blob_file(
        &headers,
        "application/octet-stream",
        &hash,
        http_cache::Policy::Immutable,
        &app,
    )
    .await
}

async fn send_blob(headers: &HeaderMap, logical: &str, hash: &str, app: &App) -> Response {
    let mime = mime_guess::from_path(logical).first_or_octet_stream();
    send_blob_file(
        headers,
        mime.essence_str(),
        hash,
        http_cache::Policy::Revalidate,
        app,
    )
    .await
}

async fn send_expiring_blob(
    headers: &HeaderMap,
    name: &str,
    logical: &str,
    hash: &str,
    app: &App,
) -> Response {
    let report = app
        .run_store({
            let name = name.to_string();
            let logical = logical.to_string();
            move |store| store.expiry_report(&name, &logical)
        })
        .await;
    let report = match report {
        Ok(report) => report,
        Err(err) => return err.into_response(),
    };
    let mut response = send_blob(headers, logical, hash, app).await;
    insert_expiry_headers(response.headers_mut(), &report);
    response
}

async fn add_target_expiry_headers(app: &App, name: &str, rel: &str, headers: &mut HeaderMap) {
    let report = app
        .run_store({
            let name = name.to_string();
            let rel = rel.to_string();
            move |store| store.expiry_report(&name, &rel)
        })
        .await;
    if let Ok(report) = report {
        insert_expiry_headers(headers, &report);
    }
}

async fn send_blob_file(
    headers: &HeaderMap,
    content_type: &str,
    hash: &str,
    policy: http_cache::Policy,
    app: &App,
) -> Response {
    let etag = format!("\"{hash}\"");
    if let Some(mut response) = http_cache::not_modified(headers, &etag, policy, None) {
        response
            .headers_mut()
            .insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
        return response;
    }

    let path = app.store.blob_path(hash);
    let size = match tokio::fs::metadata(&path).await {
        Ok(metadata) => metadata.len(),
        Err(err) => return StoreError::Io(err).into_response(),
    };
    let Ok(range) = requested_range(headers, size, &etag) else {
        return range_not_satisfiable(size, &etag, policy);
    };

    if range.is_none() && size <= STREAM_THRESHOLD {
        let bytes = app
            .run_store({
                let hash = hash.to_string();
                move |store| store.read_blob(&hash)
            })
            .await;
        return match bytes {
            Ok(bytes) => {
                let mut representation =
                    http_cache::Representation::new(bytes, "application/octet-stream");
                representation.content_type =
                    HeaderValue::from_str(content_type).expect("valid MIME type");
                representation.policy = policy;
                representation.nosniff = true;
                representation.etag = Some(etag);
                let mut response = http_cache::respond(headers, representation);
                response
                    .headers_mut()
                    .insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
                response.headers_mut().insert(
                    header::CONTENT_LENGTH,
                    HeaderValue::from_str(&size.to_string()).expect("valid content length"),
                );
                response
            }
            Err(err) => err.into_response(),
        };
    }

    let mut file = match tokio::fs::File::open(path).await {
        Ok(file) => file,
        Err(err) => return StoreError::Io(err).into_response(),
    };
    let (status, length, content_range) = match range {
        Some(range) => {
            if let Err(err) = file.seek(SeekFrom::Start(range.start)).await {
                return StoreError::Io(err).into_response();
            }
            (
                StatusCode::PARTIAL_CONTENT,
                range.len(),
                Some(format!("bytes {}-{}/{}", range.start, range.end, size)),
            )
        }
        None => (StatusCode::OK, size, None),
    };
    let stream = ReaderStream::new(file.take(length));
    let mut response = Response::new(Body::from_stream(stream));
    *response.status_mut() = status;
    let response_headers = response.headers_mut();
    response_headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(content_type).expect("valid MIME type"),
    );
    response_headers.insert(
        header::CONTENT_LENGTH,
        HeaderValue::from_str(&length.to_string()).expect("valid content length"),
    );
    response_headers.insert(
        header::ETAG,
        HeaderValue::from_str(&etag).expect("valid ETag"),
    );
    response_headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static(policy.value()),
    );
    response_headers.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    response_headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    if let Some(content_range) = content_range {
        response_headers.insert(
            header::CONTENT_RANGE,
            HeaderValue::from_str(&content_range).expect("valid content range"),
        );
    }
    response
}

#[derive(Clone, Copy)]
struct ByteRange {
    start: u64,
    end: u64,
}

impl ByteRange {
    const fn len(self) -> u64 {
        self.end - self.start + 1
    }
}

fn requested_range(headers: &HeaderMap, size: u64, etag: &str) -> Result<Option<ByteRange>, ()> {
    let Some(value) = headers.get(header::RANGE) else {
        return Ok(None);
    };
    if headers
        .get(header::IF_RANGE)
        .is_some_and(|if_range| if_range.as_bytes() != etag.as_bytes())
    {
        return Ok(None);
    }
    let value = value.to_str().map_err(|_| ())?;
    let Some(range) = value.strip_prefix("bytes=") else {
        return Ok(None);
    };
    if range.contains(',') {
        return Ok(None);
    }
    let (start, end) = range.split_once('-').ok_or(())?;
    if size == 0 {
        return Err(());
    }
    if start.is_empty() {
        let suffix = end.parse::<u64>().map_err(|_| ())?;
        if suffix == 0 {
            return Err(());
        }
        let length = suffix.min(size);
        return Ok(Some(ByteRange {
            start: size - length,
            end: size - 1,
        }));
    }
    let start = start.parse::<u64>().map_err(|_| ())?;
    if start >= size {
        return Err(());
    }
    let end = if end.is_empty() {
        size - 1
    } else {
        end.parse::<u64>().map_err(|_| ())?.min(size - 1)
    };
    if end < start {
        return Err(());
    }
    Ok(Some(ByteRange { start, end }))
}

fn range_not_satisfiable(size: u64, etag: &str, policy: http_cache::Policy) -> Response {
    let mut response = Response::new(Body::empty());
    *response.status_mut() = StatusCode::RANGE_NOT_SATISFIABLE;
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_RANGE,
        HeaderValue::from_str(&format!("bytes */{size}")).expect("valid content range"),
    );
    headers.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    headers.insert(
        header::ETAG,
        HeaderValue::from_str(etag).expect("valid ETag"),
    );
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static(policy.value()),
    );
    response
}

fn wants_unpack(headers: &HeaderMap) -> bool {
    headers.get("unpack").is_some_and(|v| {
        v.to_str().is_ok_and(|v| {
            let v = v.trim();
            v.is_empty()
                || v.eq_ignore_ascii_case("1")
                || v.eq_ignore_ascii_case("true")
                || v.eq_ignore_ascii_case("yes")
        })
    })
}

fn if_match_from(headers: &HeaderMap) -> Result<Option<String>, &'static str> {
    let Some(value) = headers.get(header::IF_MATCH) else {
        return Ok(None);
    };
    let value = value
        .to_str()
        .map_err(|_| "error: invalid If-Match header")?
        .trim();
    let value = value
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .unwrap_or(value);
    if value.contains(',')
        || value == "*"
        || !value.starts_with("blake3:")
        || value.len() != "blake3:".len() + 64
    {
        return Err("error: If-Match must contain one site tree ETag");
    }
    Ok(Some(value.to_string()))
}

fn filename_from(headers: &HeaderMap) -> Option<String> {
    headers
        .get(header::CONTENT_DISPOSITION)
        .and_then(|v| v.to_str().ok())
        .and_then(parse_filename)
}

fn parse_filename(cd: &str) -> Option<String> {
    for part in cd.split(';') {
        let part = part.trim();
        let Some(rest) = part.strip_prefix("filename=") else {
            continue;
        };
        let rest = rest.trim();
        let rest = rest
            .strip_prefix('"')
            .and_then(|s| s.strip_suffix('"'))
            .unwrap_or(rest);
        if !rest.is_empty() {
            return Some(rest.to_string());
        }
    }
    None
}

fn mutation_response(
    status: StatusCode,
    location: &str,
    body: String,
    mutation: &store::MutationResult,
) -> Response {
    let mut response = plain(status, body);
    let headers = response.headers_mut();
    headers.insert(
        header::LOCATION,
        HeaderValue::from_str(location).expect("valid public URL"),
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
    response
}

fn insert_undo_headers(headers: &mut HeaderMap, undo: &store::UndoInfo) {
    headers.insert(
        "undo-token",
        HeaderValue::from_str(&undo.token).expect("valid undo token"),
    );
    headers.insert(
        "undo-expires",
        HeaderValue::from_str(&undo.expires_at).expect("valid undo expiry"),
    );
}

fn insert_expiry_headers(headers: &mut HeaderMap, report: &expiry::ExpiryReport) {
    let Some(expires_at) = &report.effective_expires_at else {
        return;
    };
    let timestamp =
        expiry::parse_rfc3339_timestamp(expires_at).expect("stored expiry timestamp is valid");
    let http_date = timestamp
        .to_offset(time::UtcOffset::UTC)
        .format(
            &time::format_description::parse_borrowed::<2>(
                "[weekday repr:short], [day padding:zero] [month repr:short] [year] [hour]:[minute]:[second] GMT",
            )
            .expect("valid HTTP date format"),
        )
        .expect("expiry date is representable");
    headers.insert(
        header::EXPIRES,
        HeaderValue::from_str(&http_date).expect("valid Expires header"),
    );
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    if let Some(own) = &report.own_policy {
        headers.insert(
            "expiry-mode",
            HeaderValue::from_static(match own.mode {
                ExpiryMode::Relative => "relative",
                ExpiryMode::Absolute => "absolute",
                ExpiryMode::Decay => "decay",
            }),
        );
    }
}

fn plain(status: StatusCode, body: impl AsRef<str>) -> Response {
    let body = body.as_ref();
    let mut out = body.to_string();
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    (
        status,
        [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
        out,
    )
        .into_response()
}

impl IntoResponse for StoreError {
    fn into_response(self) -> Response {
        if matches!(self, Self::Unauthorized) {
            let mut response = plain(StatusCode::UNAUTHORIZED, self.to_string());
            response.headers_mut().insert(
                header::WWW_AUTHENTICATE,
                HeaderValue::from_static("Bearer realm=\"symbol\""),
            );
            response
                .headers_mut()
                .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
            return response;
        }
        if let Self::PreconditionFailed {
            revision,
            tree_hash,
        } = &self
        {
            let mut response = plain(StatusCode::PRECONDITION_FAILED, self.to_string());
            let headers = response.headers_mut();
            headers.insert(
                header::ETAG,
                HeaderValue::from_str(&format!("\"{tree_hash}\"")).expect("valid site ETag"),
            );
            headers.insert(
                "content-revision",
                HeaderValue::from_str(&revision.to_string()).expect("valid revision"),
            );
            return response;
        }
        let status = match &self {
            Self::NotFound => StatusCode::NOT_FOUND,
            Self::StaleUndo(_)
            | Self::ReservedCollision(_)
            | Self::DestinationConflict
            | Self::IdempotencyConflict
            | Self::AlreadyManaged => StatusCode::CONFLICT,
            Self::Forbidden => StatusCode::FORBIDDEN,
            Self::Upload(
                upload::UploadError::ArchiveTooLarge
                | upload::UploadError::FileTooLarge
                | upload::UploadError::TooLarge
                | upload::UploadError::TooManyFiles,
            ) => StatusCode::PAYLOAD_TOO_LARGE,
            Self::Io(_) | Self::Sqlite(_) | Self::Random(_) => StatusCode::INTERNAL_SERVER_ERROR,
            _ => StatusCode::BAD_REQUEST,
        };
        plain(status, self.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::{Body, to_bytes};
    use axum::http::Request;
    use bytes::Bytes as ByteChunk;
    use tower::ServiceExt as _;

    fn test_app(store: Store) -> App {
        App::new(store)
    }

    #[tokio::test]
    async fn stats_response_keeps_original_fields_and_adds_distributions() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::new(root.path().to_path_buf()).unwrap();
        store.put_file("one", "a.txt", b"same").unwrap();
        store.put_file("two", "b.txt", b"same").unwrap();
        let response = stats(State(test_app(store))).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CONTENT_TYPE], "application/json");
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["sites"], 2);
        assert_eq!(json["files"], 2);
        assert_eq!(json["blobs"], 1);
        assert_eq!(json["bytes"], 4);
        assert_eq!(json["logical_bytes"], 8);
        assert_eq!(json["saved_bytes"], 4);
        assert_eq!(json["file_sizes"]["median"], 4.0);
        assert_eq!(json["blob_sizes"]["median"], 4.0);
        assert!(json["serving"]["readers"]["operations"].as_u64().is_some());
    }

    #[tokio::test]
    async fn expiry_routes_return_reports_headers_and_inherited_deadlines() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::new(root.path().to_path_buf()).unwrap();
        store.put_file("hello", "index.html", b"hello").unwrap();
        let app = router(test_app(store));

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::from_bytes(b"EXPIRE").unwrap())
                    .uri("/hello")
                    .header("expiry-mode", "relative")
                    .header("expiry-in", "1h")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["expiry-mode"], "relative");
        assert!(response.headers().contains_key(header::EXPIRES));
        assert!(response.headers().contains_key("undo-token"));
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let report: expiry::ExpiryReport = serde_json::from_slice(&body).unwrap();
        assert_eq!(report.target.kind, expiry::ExpiryTargetKind::Site);

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::from_bytes(b"EXPIRE").unwrap())
                    .uri("/hello/index.html")
                    .header("expiry-mode", "never")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let report: expiry::ExpiryReport = serde_json::from_slice(&body).unwrap();
        assert!(report.own_policy.is_none());
        assert_eq!(
            report.limited_by.unwrap().kind,
            expiry::ExpiryTargetKind::Site
        );

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/hello/index.html/EXPIRES")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-cache");

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/hello/index.html")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-cache");
        assert!(response.headers().contains_key(header::EXPIRES));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn saturated_store_tasks_do_not_block_the_async_runtime() {
        let root = tempfile::tempdir().unwrap();
        let app = test_app(Store::new(root.path().to_path_buf()).unwrap());
        let capacity = app.store.blocking_capacity();
        let release = Arc::new(std::sync::Barrier::new(capacity + 1));
        let (started_tx, mut started_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut tasks = Vec::with_capacity(capacity);

        for _ in 0..capacity {
            let app = app.clone();
            let release = Arc::clone(&release);
            let started_tx = started_tx.clone();
            tasks.push(tokio::spawn(async move {
                app.run_store(move |_| {
                    started_tx.send(()).unwrap();
                    release.wait();
                    Ok(())
                })
                .await
            }));
        }
        for _ in 0..capacity {
            started_rx.recv().await.unwrap();
        }

        let extra_ran = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let extra = {
            let app = app.clone();
            let extra_ran = Arc::clone(&extra_ran);
            tokio::spawn(async move {
                app.run_store(move |_| {
                    extra_ran.store(true, std::sync::atomic::Ordering::Relaxed);
                    Ok(())
                })
                .await
            })
        };
        let runtime_marker = tokio::spawn(async { 7_u8 });
        assert_eq!(runtime_marker.await.unwrap(), 7);
        assert!(!extra_ran.load(std::sync::atomic::Ordering::Relaxed));

        release.wait();
        for task in tasks {
            task.await.unwrap().unwrap();
        }
        extra.await.unwrap().unwrap();
        assert!(extra_ran.load(std::sync::atomic::Ordering::Relaxed));
    }

    #[tokio::test]
    async fn directory_etag_changes_after_visible_publish() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::new(root.path().to_path_buf()).unwrap();
        store.put_file("hello", "a.txt", b"a").unwrap();
        let app = test_app(store.clone());
        let response = browse_dir(&app, "hello", "", true, &HeaderMap::new()).await;
        let etag = response.headers()[header::ETAG].clone();

        let mut conditional = HeaderMap::new();
        conditional.insert(header::IF_NONE_MATCH, etag.clone());
        assert_eq!(
            browse_dir(&app, "hello", "", true, &conditional)
                .await
                .status(),
            StatusCode::NOT_MODIFIED
        );

        store.put_file("hello", "b.txt", b"bb").unwrap();
        let response = browse_dir(&app, "hello", "", true, &conditional).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_ne!(response.headers()[header::ETAG], etag);
    }

    #[tokio::test]
    async fn content_addressed_blob_is_immutable_and_site_scoped() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::new(root.path().to_path_buf()).unwrap();
        store.put_file("hello", "asset.bin", b"asset").unwrap();
        store.put_file("other", "index.html", b"other").unwrap();
        let store_for_lookup = store.clone();
        let app = test_app(store);
        let store::Node::File { hash, .. } = store_for_lookup.lookup("hello", "asset.bin").unwrap()
        else {
            panic!("expected file");
        };

        let response = serve_immutable_blob(
            State(app.clone()),
            Path(("hello".to_string(), hash.clone())),
            HeaderMap::new(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()[header::CACHE_CONTROL],
            "public, max-age=31536000, immutable"
        );
        let etag = response.headers()[header::ETAG].clone();
        assert_eq!(
            to_bytes(response.into_body(), usize::MAX).await.unwrap(),
            "asset"
        );

        let mut conditional = HeaderMap::new();
        conditional.insert(header::IF_NONE_MATCH, etag);
        let response = serve_immutable_blob(
            State(app.clone()),
            Path(("hello".to_string(), hash.clone())),
            conditional,
        )
        .await;
        assert_eq!(response.status(), StatusCode::NOT_MODIFIED);

        let response = serve_immutable_blob(
            State(app),
            Path(("other".to_string(), hash)),
            HeaderMap::new(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn immutable_blob_route_does_not_shadow_site_files() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::new(root.path().to_path_buf()).unwrap();
        store
            .put_file("hello", ".blob/custom", b"site file")
            .unwrap();
        let store_for_lookup = store.clone();
        let app = router(test_app(store));
        let store::Node::File { hash, .. } =
            store_for_lookup.lookup("hello", ".blob/custom").unwrap()
        else {
            panic!("expected file");
        };

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/hello/.blob/custom")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            to_bytes(response.into_body(), usize::MAX).await.unwrap(),
            "site file"
        );

        let response = app
            .oneshot(
                Request::builder()
                    .uri(format!("/.blob/hello/{hash}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn media_responses_support_ranges_seeking_and_head() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::new(root.path().to_path_buf()).unwrap();
        store.put_file("media", "song.mp3", b"0123456789").unwrap();
        let app = router(test_app(store));

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/media/song.mp3")
                    .header(header::RANGE, "bytes=2-5")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(response.headers()[header::CONTENT_TYPE], "audio/mpeg");
        assert_eq!(response.headers()[header::CONTENT_LENGTH], "4");
        assert_eq!(response.headers()[header::CONTENT_RANGE], "bytes 2-5/10");
        assert_eq!(response.headers()[header::ACCEPT_RANGES], "bytes");
        assert_eq!(
            to_bytes(response.into_body(), usize::MAX).await.unwrap(),
            "2345"
        );

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/media/song.mp3")
                    .header(header::RANGE, "bytes=2-5")
                    .header(header::IF_RANGE, "\"stale\"")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CONTENT_LENGTH], "10");
        assert_eq!(
            to_bytes(response.into_body(), usize::MAX).await.unwrap(),
            "0123456789"
        );

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/media/song.mp3")
                    .header(header::RANGE, "bytes=99-")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::RANGE_NOT_SATISFIABLE);
        assert_eq!(response.headers()[header::CONTENT_RANGE], "bytes */10");

        let response = app
            .oneshot(
                Request::builder()
                    .method("HEAD")
                    .uri("/media/song.mp3")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CONTENT_LENGTH], "10");
        assert!(
            to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn range_parser_handles_open_suffix_and_if_range_requests() {
        let mut headers = HeaderMap::new();
        headers.insert(header::RANGE, HeaderValue::from_static("bytes=4-"));
        assert_eq!(
            requested_range(&headers, 10, "\"hash\"")
                .unwrap()
                .unwrap()
                .len(),
            6
        );
        headers.insert(header::RANGE, HeaderValue::from_static("bytes=-3"));
        let suffix = requested_range(&headers, 10, "\"hash\"").unwrap().unwrap();
        assert_eq!((suffix.start, suffix.end), (7, 9));
        headers.insert(header::IF_RANGE, HeaderValue::from_static("\"other\""));
        assert!(requested_range(&headers, 10, "\"hash\"").unwrap().is_none());
        headers.remove(header::IF_RANGE);
        headers.insert(header::RANGE, HeaderValue::from_static("bytes=0-1,4-5"));
        assert!(requested_range(&headers, 10, "\"hash\"").unwrap().is_none());
    }

    #[tokio::test]
    async fn upload_limits_reject_content_length_and_chunked_overflow() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::new(root.path().to_path_buf()).unwrap();
        let app = router(App::with_max_file_size(store.clone(), 4));

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/media/declared.bin")
                    .header(header::CONTENT_LENGTH, "5")
                    .body(Body::from("12345"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);

        let chunks = futures_util::stream::iter([
            Ok::<_, io::Error>(ByteChunk::from_static(b"123")),
            Ok(ByteChunk::from_static(b"456")),
        ]);
        let response = app
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/media/chunked.bin")
                    .body(Body::from_stream(chunks))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
        assert_eq!(store.stats().unwrap().files, 0);
        assert!(
            std::fs::read_dir(root.path().join("tmp"))
                .unwrap()
                .next()
                .is_none()
        );
    }

    #[tokio::test]
    async fn archive_download_and_pop_stream_then_remove_temporary_files() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::new(root.path().to_path_buf()).unwrap();
        store
            .put_file("media", "large.bin", &vec![9_u8; 2 * 1024 * 1024])
            .unwrap();
        let app = router(test_app(store.clone()));

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/media.tar")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let length = response.headers()[header::CONTENT_LENGTH]
            .to_str()
            .unwrap()
            .parse::<usize>()
            .unwrap();
        let archive = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert_eq!(archive.len(), length);
        assert_eq!(&archive[257..262], b"ustar");
        assert!(store.site_exists("media"));
        assert!(
            std::fs::read_dir(root.path().join("tmp"))
                .unwrap()
                .next()
                .is_none()
        );

        let response = app
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/media")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let archive = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert_eq!(&archive[..2], [0x1f, 0x8b]);
        assert!(!store.site_exists("media"));
        assert!(
            std::fs::read_dir(root.path().join("tmp"))
                .unwrap()
                .next()
                .is_none()
        );
    }

    #[tokio::test]
    async fn delete_archives_use_the_same_tar_zip_and_gzip_formats_as_get() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::new(root.path().to_path_buf()).unwrap();
        let app = router(test_app(store.clone()));
        for (name, extension, signature) in [
            ("plain", ".tar", b"ustar".as_slice()),
            ("compressed", ".tar.gz", &[0x1f, 0x8b]),
            ("zipped", ".zip", b"PK\x03\x04".as_slice()),
        ] {
            store.put_file(name, "index.html", b"hello").unwrap();
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method("DELETE")
                        .uri(format!("/{name}{extension}"))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            assert!(response.headers().contains_key("undo-token"));
            let archive = to_bytes(response.into_body(), usize::MAX).await.unwrap();
            if extension == ".tar" {
                assert_eq!(&archive[257..262], signature);
            } else {
                assert_eq!(&archive[..signature.len()], signature);
            }
            assert!(!store.site_exists(name));
        }
    }

    #[tokio::test]
    async fn copy_and_move_handlers_follow_contract() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::new(root.path().to_path_buf()).unwrap();
        store.put_file("source", "index.html", b"hello").unwrap();
        let app = router(test_app(store.clone()));

        let copied = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::from_bytes(b"COPY").unwrap())
                    .uri("/source")
                    .header("destination", "/copied")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(copied.status(), StatusCode::CREATED);
        assert_eq!(copied.headers()["location"], "http://symbol/copied/");
        assert!(copied.headers().contains_key("content-revision"));
        assert!(copied.headers().contains_key(header::ETAG));
        assert!(copied.headers().contains_key("undo-token"));

        let conflict = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::from_bytes(b"COPY").unwrap())
                    .uri("/source")
                    .header("destination", "/copied")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(conflict.status(), StatusCode::CONFLICT);

        let moved = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::from_bytes(b"MOVE").unwrap())
                    .uri("/copied")
                    .header("destination", "/renamed")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(moved.status(), StatusCode::OK);
        assert_eq!(moved.headers()["location"], "http://symbol/renamed/");
        assert!(!store.site_exists("copied"));
        assert!(store.site_exists("renamed"));
    }

    #[tokio::test]
    async fn inventory_and_put_preconditions_follow_contract() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::new(root.path().to_path_buf()).unwrap();
        store.put_file("source", "index.html", b"hello").unwrap();
        let app = router(test_app(store.clone()));
        let inventory = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/source/FILES")
                    .header(header::ACCEPT, "application/json")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(inventory.status(), StatusCode::OK);
        let baseline_etag = inventory.headers()[header::ETAG].clone();
        let baseline_revision = inventory.headers()["content-revision"].clone();
        let body = to_bytes(inventory.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["site"], "source");
        assert_eq!(
            json["content_revision"].as_u64().unwrap().to_string(),
            baseline_revision.to_str().unwrap()
        );
        assert_eq!(json["files"][0]["path"], "index.html");
        assert!(
            json["files"][0]["hash"]
                .as_str()
                .unwrap()
                .starts_with("blake3:")
        );

        let updated = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/source/index.html")
                    .header(header::IF_MATCH, baseline_etag.clone())
                    .body(Body::from("updated"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(updated.status(), StatusCode::OK);
        let current_revision = updated.headers()["content-revision"].clone();

        let drift = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/source/new.txt")
                    .header(header::IF_MATCH, baseline_etag)
                    .body(Body::from("rejected"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(drift.status(), StatusCode::PRECONDITION_FAILED);
        assert_eq!(drift.headers()["content-revision"], current_revision);
        assert!(matches!(
            store.lookup("source", "new.txt"),
            Err(StoreError::NotFound)
        ));
    }

    #[tokio::test]
    async fn generated_resource_handlers_replay_idempotent_requests() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::new(root.path().to_path_buf()).unwrap();
        store.put_file("source", "index.html", b"hello").unwrap();
        let app = router(test_app(store));
        let auto_copy = |key: &'static str| {
            Request::builder()
                .method(Method::from_bytes(b"COPY").unwrap())
                .uri("/source")
                .header("idempotency-key", key)
                .body(Body::empty())
                .unwrap()
        };
        let first = app.clone().oneshot(auto_copy("copy-retry")).await.unwrap();
        let first_location = first.headers()["location"].clone();
        let replay = app.clone().oneshot(auto_copy("copy-retry")).await.unwrap();
        assert_eq!(replay.headers()["location"], first_location);

        let unnamed_put = |body: &'static str| {
            Request::builder()
                .method("PUT")
                .uri("/")
                .header("idempotency-key", "put-retry")
                .body(Body::from(body))
                .unwrap()
        };
        let first = app.clone().oneshot(unnamed_put("same")).await.unwrap();
        let first_location = first.headers()["location"].clone();
        let replay = app.clone().oneshot(unnamed_put("same")).await.unwrap();
        assert_eq!(replay.headers()["location"], first_location);
        let mismatch = app.oneshot(unnamed_put("different")).await.unwrap();
        assert_eq!(mismatch.status(), StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn generated_managed_replay_never_returns_new_credentials() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::new(root.path().to_path_buf()).unwrap();
        let app = router(test_app(store));
        let request = || {
            Request::builder()
                .method("PUT")
                .uri("/")
                .header("idempotency-key", "managed-put-retry")
                .header("management-action", "claim")
                .header(
                    "creator-claim",
                    "sym_claim_0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
                )
                .body(Body::from("same"))
                .unwrap()
        };
        let first = app.clone().oneshot(request()).await.unwrap();
        assert_eq!(first.status(), StatusCode::CREATED);
        assert!(first.headers().contains_key("management-token"));
        let location = first.headers()["location"].clone();

        let replay = app.oneshot(request()).await.unwrap();
        assert_eq!(replay.headers()["location"], location);
        assert_eq!(replay.headers()["idempotency-replayed"], "true");
        assert!(!replay.headers().contains_key("management-token"));
        assert!(!replay.headers().contains_key("creator-claim"));
    }

    #[tokio::test]
    async fn large_chunked_upload_is_spooled_and_range_served() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::new(root.path().to_path_buf()).unwrap();
        let app = router(test_app(store.clone()));
        let chunk = ByteChunk::from(vec![7_u8; 1024 * 1024]);
        let stream =
            futures_util::stream::iter((0..51).map(move |_| Ok::<_, io::Error>(chunk.clone())));

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/media/song.mp3")
                    .body(Body::from_stream(stream))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        assert_eq!(store.stats().unwrap().bytes, 51 * 1024 * 1024);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/media/song.mp3")
                    .header(header::RANGE, "bytes=0-3")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(
            to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap()
                .as_ref(),
            &[7_u8; 4]
        );
    }

    #[tokio::test]
    async fn mutation_headers_stack_and_guarded_undo_follow_contract() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::new(root.path().to_path_buf()).unwrap();
        let app = router(test_app(store.clone()));

        let created = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/hello/index.html")
                    .body(Body::from("first"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(created.status(), StatusCode::CREATED);
        let first_token = created.headers()["undo-token"].clone();
        assert!(created.headers().contains_key("undo-expires"));
        assert_eq!(
            created.headers()["location"],
            "http://symbol/hello/index.html"
        );

        let updated = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/hello/other.txt")
                    .body(Body::from("second"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(updated.status(), StatusCode::OK);
        let latest_token = updated.headers()["undo-token"].clone();

        let stack = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/hello/UNDO")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(stack.status(), StatusCode::OK);
        let body = to_bytes(stack.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["entries"].as_array().unwrap().len(), 2);

        let stale = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::from_bytes(b"UNDO").unwrap())
                    .uri("/hello")
                    .header("undo-token", first_token)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(stale.status(), StatusCode::CONFLICT);

        let restored = app
            .oneshot(
                Request::builder()
                    .method(Method::from_bytes(b"UNDO").unwrap())
                    .uri("/hello")
                    .header("undo-token", latest_token)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(restored.status(), StatusCode::OK);
        assert!(matches!(
            store.lookup("hello", "other.txt"),
            Err(StoreError::NotFound)
        ));
        assert!(matches!(
            store.lookup("hello", "index.html"),
            Ok(store::Node::File { .. })
        ));
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn managed_mutations_authorize_before_spooling_and_rotation_is_idempotent() {
        use std::sync::atomic::{AtomicBool, Ordering};

        let root = tempfile::tempdir().unwrap();
        let store = Store::new(root.path().to_path_buf()).unwrap();
        let app = router(test_app(store.clone()));
        let created = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/secure/index.html")
                    .header("management-action", "claim")
                    .body(Body::from("initial"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(created.status(), StatusCode::CREATED);
        assert_eq!(created.headers()[header::CACHE_CONTROL], "no-store");
        let token = created.headers()["management-token"]
            .to_str()
            .unwrap()
            .to_string();
        let claim = created.headers()["creator-claim"]
            .to_str()
            .unwrap()
            .to_string();

        let polled = Arc::new(AtomicBool::new(false));
        let polled_by_body = Arc::clone(&polled);
        let body = Body::from_stream(futures_util::stream::once(async move {
            polled_by_body.store(true, Ordering::SeqCst);
            Ok::<_, io::Error>(ByteChunk::from_static(b"must not spool"))
        }));
        let unauthorized = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/secure/rejected.txt")
                    .body(body)
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            unauthorized.headers()[header::WWW_AUTHENTICATE],
            "Bearer realm=\"symbol\""
        );
        assert!(!polled.load(Ordering::SeqCst));

        let sanitized = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/secure/leak.txt")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::from(token.clone()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(sanitized.status(), StatusCode::OK);
        assert_eq!(sanitized.headers()["sanitized-management-tokens"], "1");
        let store::Node::File { hash, .. } = store.lookup("secure", "leak.txt").unwrap() else {
            panic!("expected sanitized file");
        };
        let stored = store.read_blob(&hash).unwrap();
        assert_eq!(stored.len(), token.len());
        assert!(stored.starts_with(secrets::MANAGEMENT_TOKEN_PREFIX.as_bytes()));
        assert!(!stored.windows(16).any(|window| {
            token
                .as_bytes()
                .windows(16)
                .any(|candidate| candidate == window)
        }));

        let rotated = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::from_bytes(b"MANAGE").unwrap())
                    .uri("/secure")
                    .header("management-action", "rotate")
                    .header("idempotency-key", "rotation-1")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .header("creator-claim", &claim)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(rotated.status(), StatusCode::OK);
        let rotated_token = rotated.headers()["management-token"]
            .to_str()
            .unwrap()
            .to_string();
        assert_ne!(rotated_token, token);

        let replay = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::from_bytes(b"MANAGE").unwrap())
                    .uri("/secure")
                    .header("management-action", "rotate")
                    .header("idempotency-key", "rotation-1")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .header("creator-claim", claim)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(replay.status(), StatusCode::OK);
        assert_eq!(replay.headers()["idempotency-replayed"], "true");
        assert!(!replay.headers().contains_key("management-token"));

        let stale = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/secure/leak.txt")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(stale.status(), StatusCode::UNAUTHORIZED);

        let deleted = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/secure/leak.txt")
                    .header(header::AUTHORIZATION, format!("Bearer {rotated_token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(deleted.status(), StatusCode::OK);
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn receipt_claim_copy_isolation_move_and_managed_delete_undo_follow_contract() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::new(root.path().to_path_buf()).unwrap();
        let app = router(test_app(store.clone()));
        let created = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/source/index.html")
                    .body(Body::from("source"))
                    .unwrap(),
            )
            .await
            .unwrap();
        let claim = created.headers()["creator-claim"].to_str().unwrap();
        let claimed = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::from_bytes(b"MANAGE").unwrap())
                    .uri("/source")
                    .header("management-action", "claim")
                    .header("creator-claim", claim)
                    .header("idempotency-key", "claim-1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(claimed.status(), StatusCode::OK);
        let token = claimed.headers()["management-token"]
            .to_str()
            .unwrap()
            .to_string();

        let copied = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::from_bytes(b"COPY").unwrap())
                    .uri("/source")
                    .header("destination", "/public-copy")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(copied.status(), StatusCode::CREATED);
        assert!(!copied.headers().contains_key("management-token"));
        assert!(copied.headers().contains_key("creator-claim"));

        let managed_copy = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::from_bytes(b"COPY").unwrap())
                    .uri("/source")
                    .header("destination", "/managed-copy")
                    .header("management-action", "claim")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let copy_token = managed_copy.headers()["management-token"]
            .to_str()
            .unwrap()
            .to_string();
        assert_ne!(copy_token, token);

        let moved = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::from_bytes(b"MOVE").unwrap())
                    .uri("/managed-copy")
                    .header("destination", "/moved-copy")
                    .header(header::AUTHORIZATION, format!("Bearer {copy_token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(moved.status(), StatusCode::OK);
        store
            .authorize_mutation(
                "moved-copy",
                Some(&ManagementToken::parse(&copy_token).unwrap()),
            )
            .unwrap();

        let deleted = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/moved-copy")
                    .header(header::AUTHORIZATION, format!("Bearer {copy_token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let undo = deleted.headers()["undo-token"].clone();
        let rejected = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::from_bytes(b"UNDO").unwrap())
                    .uri("/moved-copy")
                    .header("undo-token", undo.clone())
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(rejected.status(), StatusCode::UNAUTHORIZED);
        let restored = app
            .oneshot(
                Request::builder()
                    .method(Method::from_bytes(b"UNDO").unwrap())
                    .uri("/moved-copy")
                    .header("undo-token", undo)
                    .header(header::AUTHORIZATION, format!("Bearer {copy_token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(restored.status(), StatusCode::OK);
        store
            .authorize_mutation(
                "moved-copy",
                Some(&ManagementToken::parse(&copy_token).unwrap()),
            )
            .unwrap();
    }

    #[tokio::test]
    async fn trusted_proxy_identity_requires_an_allowlisted_socket_peer() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::new(root.path().to_path_buf()).unwrap();
        let provider = IdentityProvider::TrustedProxy {
            principal_header: HeaderName::from_static("x-authenticated-user"),
            peers: Arc::from([IpAddr::from([127, 0, 0, 1])]),
        };
        let app = router(App::with_options(
            store,
            DEFAULT_MAX_FILE_SIZE,
            "http://symbol".into(),
            provider,
        ));
        let mut create = Request::builder()
            .method("PUT")
            .uri("/principal/index.html")
            .header("x-authenticated-user", "user@example.test")
            .body(Body::from("principal"))
            .unwrap();
        create
            .extensions_mut()
            .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 12345))));
        let created = app.clone().oneshot(create).await.unwrap();
        assert_eq!(created.status(), StatusCode::CREATED);
        assert!(!created.headers().contains_key("creator-claim"));

        let mut claim = Request::builder()
            .method(Method::from_bytes(b"MANAGE").unwrap())
            .uri("/principal")
            .header("management-action", "claim")
            .header("x-authenticated-user", "user@example.test")
            .body(Body::empty())
            .unwrap();
        claim
            .extensions_mut()
            .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 12346))));
        let claimed = app.clone().oneshot(claim).await.unwrap();
        assert_eq!(claimed.status(), StatusCode::OK);
        assert!(claimed.headers().contains_key("management-token"));

        let forged = app
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/untrusted/index.html")
                    .header("x-authenticated-user", "user@example.test")
                    .header(INTERNAL_CREATOR_HEADER, "forged")
                    .body(Body::from("untrusted"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(forged.headers().contains_key("creator-claim"));
    }
}
