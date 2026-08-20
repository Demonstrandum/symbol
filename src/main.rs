macro_rules! static_asset {
    ($name:literal) => {
        include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/static/", $name))
    };
}

mod blob_store;
mod browse;
mod http_cache;
mod name;
mod page;
mod pathutil;
mod store;
mod upload;

use std::collections::HashMap;
use std::io::{self, SeekFrom};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::get;
use axum::{Json, Router};
use clap::Parser;
use futures_util::StreamExt as _;
use store::{ArchiveFormat, Store, StoreError};
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

#[derive(Parser, Debug)]
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
}

#[derive(Clone)]
struct App {
    store: Store,
    store_tasks: Arc<Semaphore>,
    hashes: Arc<Mutex<HashMap<String, String>>>,
    max_file_size: u64,
}

struct TemporaryUpload {
    path: PathBuf,
}

#[derive(Clone, Copy)]
enum UploadLimitKind {
    Archive,
    File,
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
        Self::with_max_file_size(store, DEFAULT_MAX_FILE_SIZE)
    }

    fn with_max_file_size(store: Store, max_file_size: u64) -> Self {
        Self {
            store_tasks: Arc::new(Semaphore::new(store.blocking_capacity())),
            store,
            hashes: Arc::new(Mutex::new(HashMap::new())),
            max_file_size,
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
    let store = Store::new(args.root).expect("create data dir");
    let app = router(App::with_max_file_size(store, args.max_file_size));
    let listener = TcpListener::bind(&args.bind)
        .await
        .unwrap_or_else(|err| panic!("bind {}: {err}", args.bind));
    tracing::info!("listening on {}", args.bind);
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown())
        .await
        .expect("server");
}

fn router(app: App) -> Router {
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
        .route(
            "/{name}/",
            get(serve_index).put(put_site).delete(delete_site),
        )
        .route("/.blob/{name}/{hash}", get(serve_immutable_blob))
        .route(
            "/{name}/{*path}",
            get(serve_path).put(put_file).delete(delete_file),
        )
        .route(
            "/{name}",
            get(redirect_site).put(put_site).delete(delete_site),
        )
        .layer(TraceLayer::new_for_http())
        .with_state(app)
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

fn base_url(headers: &HeaderMap) -> String {
    let proto = headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("http");
    let host = headers
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("symbol");
    format!("{proto}://{host}")
}

fn script_body(template: &str, headers: &HeaderMap) -> String {
    template.replace("__HOST__", &base_url(headers))
}

fn render_script(template: &str, headers: &HeaderMap) -> Response {
    let body = script_body(template, headers);
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

async fn docs_hash(State(app): State<App>, headers: HeaderMap) -> Response {
    let host = base_url(&headers);
    let body = page::render_plain(&host);
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

async fn docs(headers: HeaderMap) -> Response {
    page::render(&headers, &base_url(&headers), page::negotiate(&headers))
}

async fn install_sh(headers: HeaderMap) -> Response {
    render_script(INSTALL_SH, &headers)
}

async fn symbol_sh(headers: HeaderMap) -> Response {
    render_script(SYMBOL_SH, &headers)
}

async fn list_sites(State(app): State<App>, headers: HeaderMap) -> Response {
    match app.run_store(|store| store.list_sites()).await {
        Ok(names) => browse::sites(&headers, &names),
        Err(err) => err.into_response(),
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

async fn publish(app: &App, wanted: Option<String>, headers: &HeaderMap, body: Body) -> Response {
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
    let result = if unpack {
        publish_archive(app, wanted, filename, kind, temporary.path.clone()).await
    } else {
        let stored_name = filename
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| kind.default_filename().to_string());
        app.run_store({
            let temporary = temporary.path.clone();
            move |store| store.publish_uploaded_file(wanted.as_deref(), &stored_name, temporary)
        })
        .await
    };
    match result {
        Ok((name, n)) => {
            let url = format!("{}/{name}/", base_url(headers));
            (
                StatusCode::CREATED,
                [
                    (header::CONTENT_TYPE, "text/plain; charset=utf-8"),
                    (header::LOCATION, url.as_str()),
                ],
                format!("ok {name} {url} ({n} files)\n"),
            )
                .into_response()
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
) -> Result<(String, usize), StoreError> {
    let bytes = tokio::fs::read(path).await?;
    app.run_store(move |store| {
        store.publish(wanted.as_deref(), true, &bytes, kind, filename.as_deref())
    })
    .await
}

async fn put_file(
    State(app): State<App>,
    Path((name, path)): Path<(String, String)>,
    headers: HeaderMap,
    body: Body,
) -> Response {
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
            move |store| store.put_uploaded_file(&name, &path, temporary)
        })
        .await;
    match result {
        Ok(()) => plain(StatusCode::CREATED, format!("ok /{name}/{path}")),
        Err(err) => err.into_response(),
    }
}

async fn delete_site(State(app): State<App>, Path(name): Path<String>) -> Response {
    let temporary = TemporaryUpload {
        path: app.store.upload_path(),
    };
    let result = app
        .run_store({
            let name = name.clone();
            let path = temporary.path.clone();
            move |store| store.pop_site_to_path(&name, &path)
        })
        .await;
    match result {
        Ok(size) => {
            archive_response(
                temporary,
                size,
                "application/gzip",
                format!("attachment; filename=\"{name}.tar.gz\""),
            )
            .await
        }
        Err(err) => err.into_response(),
    }
}

async fn delete_file(
    State(app): State<App>,
    Path((name, path)): Path<(String, String)>,
) -> Response {
    let result = app
        .run_store({
            let name = name.clone();
            let path = path.clone();
            move |store| store.delete_file(&name, &path)
        })
        .await;
    match result {
        Ok(()) => plain(StatusCode::OK, format!("deleted {name}/{path}")),
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
            archive_response(
                temporary,
                size,
                request.content_type,
                format!(
                    "attachment; filename=\"{}{}\"",
                    request.name, request.extension
                ),
            )
            .await
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
                    return send_blob(headers, &logical, &hash, app).await;
                }
                Ok(None) => {}
                Err(err) => return err.into_response(),
            }
            browse_dir(app, name, rel, false, headers).await
        }
        Ok(store::Node::File { logical, hash }) => send_blob(headers, &logical, &hash, app).await,
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
        let status = match &self {
            Self::NotFound => StatusCode::NOT_FOUND,
            Self::Upload(
                upload::UploadError::ArchiveTooLarge
                | upload::UploadError::FileTooLarge
                | upload::UploadError::TooLarge
                | upload::UploadError::TooManyFiles,
            ) => StatusCode::PAYLOAD_TOO_LARGE,
            Self::Io(_) | Self::Sqlite(_) => StatusCode::INTERNAL_SERVER_ERROR,
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
}
