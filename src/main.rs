mod browse;
mod name;
mod page;
mod pathutil;
mod store;
mod upload;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::get;
use axum::{Json, Router};
use clap::Parser;
use store::{ArchiveFormat, Store, StoreError};
use tokio::net::TcpListener;
use tower_http::trace::TraceLayer;

const MAX_BODY: usize = 50 * 1024 * 1024;
const INSTALL_SH: &str = include_str!("../ops/install.sh");
const SYMBOL_SH: &str = include_str!("../ops/symbol.sh");

#[derive(Parser, Debug)]
#[command(name = "symbol", about = "Tiny static-site hosting for the tailnet")]
struct Args {
    #[arg(long, default_value = "127.0.0.1:4340", env = "SYMBOL_BIND")]
    bind: String,
    #[arg(long, default_value = "/var/lib/symbol", env = "SYMBOL_ROOT")]
    root: PathBuf,
}

#[derive(Clone)]
struct App {
    store: Store,
    hashes: Arc<Mutex<HashMap<String, String>>>,
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
    let app = router(App {
        store,
        hashes: Arc::new(Mutex::new(HashMap::new())),
    });
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
        .route(
            "/{name}/{*path}",
            get(serve_path).put(put_file).delete(delete_file),
        )
        .route(
            "/{name}",
            get(redirect_site).put(put_site).delete(delete_site),
        )
        .layer(DefaultBodyLimit::max(MAX_BODY))
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
    (
        [
            (header::CONTENT_TYPE, "text/x-shellscript; charset=utf-8"),
            (header::CACHE_CONTROL, "no-cache"),
        ],
        body,
    )
        .into_response()
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
    match app.store.stats() {
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

fn send_hash(app: &App, name: &str, rel: &str) -> Response {
    let lookup = if rel.is_empty() {
        ["index.html", "index.htm"].iter().find_map(|index| {
            app.store
                .child_blob(name, "", index)
                .ok()
                .and_then(|node| match node {
                    store::Node::File { hash, .. } => Some(hash),
                    store::Node::Dir => None,
                })
        })
    } else {
        match app.store.lookup(name, rel) {
            Ok(store::Node::File { hash, .. }) => Some(hash),
            Ok(store::Node::Dir) | Err(StoreError::NotFound) => None,
            Err(err) => return err.into_response(),
        }
    };
    lookup.map_or_else(
        || StoreError::NotFound.into_response(),
        |hash| plain(StatusCode::OK, hash),
    )
}

async fn docs(headers: HeaderMap) -> Response {
    page::render(&base_url(&headers), page::negotiate(&headers))
}

async fn install_sh(headers: HeaderMap) -> Response {
    render_script(INSTALL_SH, &headers)
}

async fn symbol_sh(headers: HeaderMap) -> Response {
    render_script(SYMBOL_SH, &headers)
}

async fn list_sites(State(app): State<App>, headers: HeaderMap) -> Response {
    match app.store.list_sites() {
        Ok(names) => browse::sites(&headers, &names),
        Err(err) => err.into_response(),
    }
}

async fn put_site_unnamed(State(app): State<App>, headers: HeaderMap, body: Bytes) -> Response {
    publish(&app, None, &headers, &body)
}

async fn put_site(
    State(app): State<App>,
    Path(name): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    publish(&app, Some(name.as_str()), &headers, &body)
}

fn publish(app: &App, wanted: Option<&str>, headers: &HeaderMap, body: &Bytes) -> Response {
    let filename = filename_from(headers);
    let ctype = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok());
    let kind = upload::sniff(body, ctype, filename.as_deref());
    match app.store.publish(
        wanted,
        wants_unpack(headers),
        body,
        kind,
        filename.as_deref(),
    ) {
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

async fn put_file(
    State(app): State<App>,
    Path((name, path)): Path<(String, String)>,
    body: Bytes,
) -> Response {
    match app.store.put_file(&name, &path, &body) {
        Ok(()) => plain(StatusCode::CREATED, format!("ok /{name}/{path}")),
        Err(err) => err.into_response(),
    }
}

async fn delete_site(State(app): State<App>, Path(name): Path<String>) -> Response {
    match app.store.pop_site(&name) {
        Ok(bytes) => (
            [
                (header::CONTENT_TYPE, "application/gzip".to_string()),
                (
                    header::CONTENT_DISPOSITION,
                    format!("attachment; filename=\"{name}.tar.gz\""),
                ),
                (header::CACHE_CONTROL, "no-cache".to_string()),
                (header::X_CONTENT_TYPE_OPTIONS, "nosniff".to_string()),
            ],
            bytes,
        )
            .into_response(),
        Err(err) => err.into_response(),
    }
}

async fn delete_file(
    State(app): State<App>,
    Path((name, path)): Path<(String, String)>,
) -> Response {
    match app.store.delete_file(&name, &path) {
        Ok(()) => plain(StatusCode::OK, format!("deleted {name}/{path}")),
        Err(err) => err.into_response(),
    }
}

#[derive(Clone, Copy)]
struct ArchiveDownload<'a> {
    name: &'a str,
    format: ArchiveFormat,
    extension: &'static str,
    content_type: &'static str,
}

fn archive_download(path: &str) -> Option<ArchiveDownload<'_>> {
    [
        (".tar.gz", ArchiveFormat::TarGz, "application/gzip"),
        (".tar", ArchiveFormat::Tar, "application/x-tar"),
        (".zip", ArchiveFormat::Zip, "application/zip"),
    ]
    .into_iter()
    .find_map(|(extension, format, content_type)| {
        path.strip_suffix(extension).map(|name| ArchiveDownload {
            name,
            format,
            extension,
            content_type,
        })
    })
}

fn download_site(app: &App, request: ArchiveDownload<'_>) -> Response {
    match app.store.pack_site(request.name, request.format) {
        Ok(bytes) => (
            [
                (header::CONTENT_TYPE, request.content_type.to_string()),
                (
                    header::CONTENT_DISPOSITION,
                    format!(
                        "attachment; filename=\"{}{}\"",
                        request.name, request.extension
                    ),
                ),
                (header::CACHE_CONTROL, "no-cache".to_string()),
                (header::X_CONTENT_TYPE_OPTIONS, "nosniff".to_string()),
            ],
            bytes,
        )
            .into_response(),
        Err(err) => err.into_response(),
    }
}

async fn redirect_site(State(app): State<App>, Path(name): Path<String>) -> Response {
    if let Some(request) = archive_download(&name) {
        return download_site(&app, request);
    }
    if app.store.site_exists(&name) {
        Redirect::temporary(&format!("/{name}/")).into_response()
    } else {
        StoreError::NotFound.into_response()
    }
}

async fn browse_root(
    State(app): State<App>,
    Path(name): Path<String>,
    headers: HeaderMap,
) -> Response {
    browse_dir(&app, &name, "", true, &headers)
}

async fn browse_path(
    State(app): State<App>,
    Path((name, path)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    let rel = path.trim_end_matches('/');
    match app.store.lookup(&name, rel) {
        Ok(store::Node::Dir) => {
            if !path.is_empty() && !path.ends_with('/') {
                return Redirect::temporary(&format!("/{name}/FILES/{path}/")).into_response();
            }
            browse_dir(&app, &name, rel, true, &headers)
        }
        Ok(store::Node::File { .. }) => {
            Redirect::temporary(&format!("/{name}/{rel}")).into_response()
        }
        Err(err) => err.into_response(),
    }
}

fn browse_dir(app: &App, name: &str, rel: &str, files_view: bool, headers: &HeaderMap) -> Response {
    match app.store.list_dir(name, rel) {
        Ok(entries) => browse::listing(headers, name, rel, &entries, files_view),
        Err(err) => err.into_response(),
    }
}

async fn serve_index(
    State(app): State<App>,
    Path(name): Path<String>,
    headers: HeaderMap,
) -> Response {
    serve_from(&app, &name, "", &headers)
}

async fn serve_path(
    State(app): State<App>,
    Path((name, path)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    if let Some(rel) = strip_hash_path(&path) {
        return send_hash(&app, &name, rel);
    }
    serve_from(&app, &name, &path, &headers)
}

fn serve_from(app: &App, name: &str, rel: &str, headers: &HeaderMap) -> Response {
    match app.store.lookup(name, rel) {
        Ok(store::Node::Dir) => {
            if !rel.is_empty() && !rel.ends_with('/') {
                return Redirect::temporary(&format!("/{name}/{rel}/")).into_response();
            }
            for index in ["index.html", "index.htm"] {
                if let Ok(store::Node::File { logical, hash }) =
                    app.store.child_blob(name, rel, index)
                {
                    return send_blob(headers, &logical, &hash, app);
                }
            }
            browse_dir(app, name, rel, false, headers)
        }
        Ok(store::Node::File { logical, hash }) => send_blob(headers, &logical, &hash, app),
        Err(err) => err.into_response(),
    }
}

fn send_blob(headers: &HeaderMap, logical: &str, hash: &str, app: &App) -> Response {
    let etag = format!("\"{hash}\"");
    if headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.split(',').any(|t| t.trim() == etag))
    {
        return (
            StatusCode::NOT_MODIFIED,
            [
                (header::ETAG, etag),
                (header::CACHE_CONTROL, "no-cache".to_string()),
            ],
        )
            .into_response();
    }
    let mime = mime_guess::from_path(logical).first_or_octet_stream();
    match app.store.read_blob(hash) {
        Ok(bytes) => (
            [
                (header::CONTENT_TYPE, mime.essence_str().to_string()),
                (header::ETAG, etag),
                (header::CACHE_CONTROL, "no-cache".to_string()),
                (header::X_CONTENT_TYPE_OPTIONS, "nosniff".to_string()),
            ],
            bytes,
        )
            .into_response(),
        Err(err) => err.into_response(),
    }
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
            Self::Upload(upload::UploadError::TooLarge | upload::UploadError::TooManyFiles) => {
                StatusCode::PAYLOAD_TOO_LARGE
            }
            Self::Io(_) | Self::Sqlite(_) => StatusCode::INTERNAL_SERVER_ERROR,
            _ => StatusCode::BAD_REQUEST,
        };
        plain(status, self.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;

    #[tokio::test]
    async fn stats_response_keeps_original_fields_and_adds_distributions() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::new(root.path().to_path_buf()).unwrap();
        store.put_file("one", "a.txt", b"same").unwrap();
        store.put_file("two", "b.txt", b"same").unwrap();
        let response = stats(State(App {
            store,
            hashes: Arc::new(Mutex::new(HashMap::new())),
        }))
        .await;
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
    }
}
