use super::*;
use axum::body::Body;
use axum::http::Request;
use tower::ServiceExt as _;

#[derive(Clone, Copy)]
enum Fixture {
    Empty,
    Site,
    Undo,
    Managed,
}

#[derive(Clone, Copy)]
enum Target {
    Literal(&'static str),
    Blob,
}

#[derive(Clone, Copy)]
struct HeaderExpectation {
    name: &'static str,
    value: Option<&'static str>,
}

#[derive(Clone, Copy)]
struct Probe {
    endpoint: &'static str,
    fixture: Fixture,
    method: &'static str,
    target: Target,
    request_headers: &'static [(&'static str, &'static str)],
    body: &'static [u8],
    status: u16,
    response_headers: &'static [HeaderExpectation],
}

const fn header(name: &'static str) -> HeaderExpectation {
    HeaderExpectation { name, value: None }
}

const fn header_value(name: &'static str, value: &'static str) -> HeaderExpectation {
    HeaderExpectation {
        name,
        value: Some(value),
    }
}

const READ_HEADERS: &[HeaderExpectation] = &[
    header("content-type"),
    header("content-length"),
    header("etag"),
    header("cache-control"),
];
const MUTATION_HEADERS: &[HeaderExpectation] = &[
    header("location"),
    header("etag"),
    header("content-revision"),
    header("undo-token"),
    header("undo-expires"),
];
const ARCHIVE_HEADERS: &[HeaderExpectation] = &[
    header("content-type"),
    header("content-length"),
    header("content-disposition"),
    header_value("cache-control", "no-cache"),
];
const PLAIN_HEADER: &[HeaderExpectation] =
    &[header_value("content-type", "text/plain; charset=utf-8")];
const JSON_HEADER: &[HeaderExpectation] = &[header_value("content-type", "application/json")];
const CACHE_JSON_HEADERS: &[HeaderExpectation] = &[
    header_value("content-type", "application/json"),
    header_value("cache-control", "no-cache"),
];
const NO_REQUEST_HEADERS: &[(&str, &str)] = &[];

const SUCCESS_PROBES: &[Probe] = &[
    Probe {
        endpoint: "docs",
        fixture: Fixture::Empty,
        method: "GET",
        target: Target::Literal("/"),
        request_headers: NO_REQUEST_HEADERS,
        body: b"",
        status: 200,
        response_headers: &[
            header("content-type"),
            header("etag"),
            header_value("cache-control", "no-cache"),
        ],
    },
    Probe {
        endpoint: "unnamed put",
        fixture: Fixture::Empty,
        method: "PUT",
        target: Target::Literal("/"),
        request_headers: &[("content-type", "text/html")],
        body: b"<h1>unnamed</h1>",
        status: 201,
        response_headers: MUTATION_HEADERS,
    },
    Probe {
        endpoint: "docs hash",
        fixture: Fixture::Empty,
        method: "GET",
        target: Target::Literal("/HASH"),
        request_headers: NO_REQUEST_HEADERS,
        body: b"",
        status: 200,
        response_headers: PLAIN_HEADER,
    },
    Probe {
        endpoint: "stats",
        fixture: Fixture::Empty,
        method: "GET",
        target: Target::Literal("/STATS"),
        request_headers: NO_REQUEST_HEADERS,
        body: b"",
        status: 200,
        response_headers: JSON_HEADER,
    },
    Probe {
        endpoint: "installer",
        fixture: Fixture::Empty,
        method: "GET",
        target: Target::Literal("/install.sh"),
        request_headers: NO_REQUEST_HEADERS,
        body: b"",
        status: 200,
        response_headers: &[
            header("content-type"),
            header("etag"),
            header_value("cache-control", "no-cache"),
        ],
    },
    Probe {
        endpoint: "installer hash",
        fixture: Fixture::Empty,
        method: "GET",
        target: Target::Literal("/install.sh/HASH"),
        request_headers: NO_REQUEST_HEADERS,
        body: b"",
        status: 200,
        response_headers: PLAIN_HEADER,
    },
    Probe {
        endpoint: "client",
        fixture: Fixture::Empty,
        method: "GET",
        target: Target::Literal("/symbol.sh"),
        request_headers: NO_REQUEST_HEADERS,
        body: b"",
        status: 200,
        response_headers: &[
            header("content-type"),
            header("etag"),
            header_value("cache-control", "no-cache"),
        ],
    },
    Probe {
        endpoint: "client hash",
        fixture: Fixture::Empty,
        method: "GET",
        target: Target::Literal("/symbol.sh/HASH"),
        request_headers: NO_REQUEST_HEADERS,
        body: b"",
        status: 200,
        response_headers: PLAIN_HEADER,
    },
    Probe {
        endpoint: "site listing",
        fixture: Fixture::Site,
        method: "GET",
        target: Target::Literal("/FILES"),
        request_headers: &[("accept", "application/json")],
        body: b"",
        status: 200,
        response_headers: &[
            header("content-type"),
            header("etag"),
            header_value("cache-control", "no-cache"),
        ],
    },
    Probe {
        endpoint: "site redirect",
        fixture: Fixture::Site,
        method: "GET",
        target: Target::Literal("/hello"),
        request_headers: NO_REQUEST_HEADERS,
        body: b"",
        status: 307,
        response_headers: &[header_value("location", "/hello/")],
    },
    Probe {
        endpoint: "site index",
        fixture: Fixture::Site,
        method: "GET",
        target: Target::Literal("/hello/"),
        request_headers: NO_REQUEST_HEADERS,
        body: b"",
        status: 200,
        response_headers: READ_HEADERS,
    },
    Probe {
        endpoint: "site put",
        fixture: Fixture::Site,
        method: "PUT",
        target: Target::Literal("/hello"),
        request_headers: &[("content-type", "text/html")],
        body: b"<h1>updated</h1>",
        status: 200,
        response_headers: MUTATION_HEADERS,
    },
    Probe {
        endpoint: "site pop",
        fixture: Fixture::Site,
        method: "DELETE",
        target: Target::Literal("/hello"),
        request_headers: NO_REQUEST_HEADERS,
        body: b"",
        status: 200,
        response_headers: &[
            header("content-type"),
            header("content-length"),
            header("content-disposition"),
            header("undo-token"),
            header("undo-expires"),
        ],
    },
    Probe {
        endpoint: "site copy",
        fixture: Fixture::Site,
        method: "COPY",
        target: Target::Literal("/hello"),
        request_headers: &[("destination", "/copy")],
        body: b"",
        status: 201,
        response_headers: MUTATION_HEADERS,
    },
    Probe {
        endpoint: "site move",
        fixture: Fixture::Site,
        method: "MOVE",
        target: Target::Literal("/hello"),
        request_headers: &[("destination", "/moved")],
        body: b"",
        status: 200,
        response_headers: MUTATION_HEADERS,
    },
    Probe {
        endpoint: "site undo",
        fixture: Fixture::Undo,
        method: "UNDO",
        target: Target::Literal("/hello"),
        request_headers: NO_REQUEST_HEADERS,
        body: b"",
        status: 200,
        response_headers: PLAIN_HEADER,
    },
    Probe {
        endpoint: "site expire",
        fixture: Fixture::Site,
        method: "EXPIRE",
        target: Target::Literal("/hello"),
        request_headers: &[("expiry-mode", "relative"), ("expiry-in", "1h")],
        body: b"",
        status: 200,
        response_headers: &[
            header("content-type"),
            header_value("cache-control", "no-cache"),
            header("expires"),
            header_value("expiry-mode", "relative"),
            header("undo-token"),
            header("undo-expires"),
        ],
    },
    Probe {
        endpoint: "site management",
        fixture: Fixture::Site,
        method: "MANAGE",
        target: Target::Literal("/hello"),
        request_headers: &[("management-action", "status")],
        body: b"",
        status: 200,
        response_headers: &[
            header("content-type"),
            header_value("cache-control", "no-store"),
        ],
    },
    Probe {
        endpoint: "site file",
        fixture: Fixture::Site,
        method: "GET",
        target: Target::Literal("/hello/assets/app.js"),
        request_headers: &[("range", "bytes=0-1")],
        body: b"",
        status: 206,
        response_headers: &[
            header("content-type"),
            header("content-length"),
            header("content-range"),
            header("accept-ranges"),
            header("etag"),
            header("cache-control"),
        ],
    },
    Probe {
        endpoint: "file put",
        fixture: Fixture::Site,
        method: "PUT",
        target: Target::Literal("/hello/new.txt"),
        request_headers: &[("content-type", "text/plain")],
        body: b"new",
        status: 200,
        response_headers: MUTATION_HEADERS,
    },
    Probe {
        endpoint: "file delete",
        fixture: Fixture::Site,
        method: "DELETE",
        target: Target::Literal("/hello/assets/app.js"),
        request_headers: NO_REQUEST_HEADERS,
        body: b"",
        status: 200,
        response_headers: &[
            header("content-type"),
            header("undo-token"),
            header("undo-expires"),
        ],
    },
    Probe {
        endpoint: "file expire",
        fixture: Fixture::Site,
        method: "EXPIRE",
        target: Target::Literal("/hello/assets/app.js"),
        request_headers: &[("expiry-mode", "relative"), ("expiry-in", "1h")],
        body: b"",
        status: 200,
        response_headers: &[
            header("content-type"),
            header_value("cache-control", "no-cache"),
            header("expires"),
            header_value("expiry-mode", "relative"),
            header("undo-token"),
            header("undo-expires"),
        ],
    },
    Probe {
        endpoint: "archive get",
        fixture: Fixture::Site,
        method: "GET",
        target: Target::Literal("/hello.tar.gz"),
        request_headers: NO_REQUEST_HEADERS,
        body: b"",
        status: 200,
        response_headers: ARCHIVE_HEADERS,
    },
    Probe {
        endpoint: "archive pop",
        fixture: Fixture::Site,
        method: "DELETE",
        target: Target::Literal("/hello.tar.gz"),
        request_headers: NO_REQUEST_HEADERS,
        body: b"",
        status: 200,
        response_headers: &[
            header("content-type"),
            header("content-length"),
            header("content-disposition"),
            header("undo-token"),
            header("undo-expires"),
        ],
    },
    Probe {
        endpoint: "files inventory",
        fixture: Fixture::Site,
        method: "GET",
        target: Target::Literal("/hello/FILES"),
        request_headers: &[("accept", "application/json")],
        body: b"",
        status: 200,
        response_headers: &[
            header_value("content-type", "application/json"),
            header("etag"),
            header("content-revision"),
            header_value("cache-control", "no-cache"),
        ],
    },
    Probe {
        endpoint: "files subtree",
        fixture: Fixture::Site,
        method: "GET",
        target: Target::Literal("/hello/FILES/assets/"),
        request_headers: &[("accept", "application/json")],
        body: b"",
        status: 200,
        response_headers: &[
            header("content-type"),
            header("etag"),
            header_value("cache-control", "no-cache"),
        ],
    },
    Probe {
        endpoint: "file hash",
        fixture: Fixture::Site,
        method: "GET",
        target: Target::Literal("/hello/assets/app.js/HASH"),
        request_headers: NO_REQUEST_HEADERS,
        body: b"",
        status: 200,
        response_headers: PLAIN_HEADER,
    },
    Probe {
        endpoint: "undo stack",
        fixture: Fixture::Undo,
        method: "GET",
        target: Target::Literal("/hello/UNDO"),
        request_headers: NO_REQUEST_HEADERS,
        body: b"",
        status: 200,
        response_headers: CACHE_JSON_HEADERS,
    },
    Probe {
        endpoint: "expiry inventory",
        fixture: Fixture::Site,
        method: "GET",
        target: Target::Literal("/hello/EXPIRES"),
        request_headers: NO_REQUEST_HEADERS,
        body: b"",
        status: 200,
        response_headers: CACHE_JSON_HEADERS,
    },
    Probe {
        endpoint: "expiry target",
        fixture: Fixture::Site,
        method: "GET",
        target: Target::Literal("/hello/assets/app.js/EXPIRES"),
        request_headers: NO_REQUEST_HEADERS,
        body: b"",
        status: 200,
        response_headers: CACHE_JSON_HEADERS,
    },
    Probe {
        endpoint: "immutable blob",
        fixture: Fixture::Site,
        method: "GET",
        target: Target::Blob,
        request_headers: &[("range", "bytes=0-1")],
        body: b"",
        status: 206,
        response_headers: &[
            header("content-type"),
            header("content-length"),
            header("content-range"),
            header("accept-ranges"),
            header("etag"),
            header_value("cache-control", "public, max-age=31536000, immutable"),
        ],
    },
];

const ERROR_PROBES: &[Probe] = &[
    Probe {
        endpoint: "unnamed put",
        fixture: Fixture::Empty,
        method: "PUT",
        target: Target::Literal("/"),
        request_headers: NO_REQUEST_HEADERS,
        body: b"",
        status: 400,
        response_headers: PLAIN_HEADER,
    },
    Probe {
        endpoint: "site redirect",
        fixture: Fixture::Empty,
        method: "GET",
        target: Target::Literal("/missing"),
        request_headers: NO_REQUEST_HEADERS,
        body: b"",
        status: 404,
        response_headers: PLAIN_HEADER,
    },
    Probe {
        endpoint: "site index",
        fixture: Fixture::Empty,
        method: "GET",
        target: Target::Literal("/missing/"),
        request_headers: NO_REQUEST_HEADERS,
        body: b"",
        status: 404,
        response_headers: PLAIN_HEADER,
    },
    Probe {
        endpoint: "site put",
        fixture: Fixture::Site,
        method: "PUT",
        target: Target::Literal("/hello"),
        request_headers: NO_REQUEST_HEADERS,
        body: b"",
        status: 400,
        response_headers: PLAIN_HEADER,
    },
    Probe {
        endpoint: "site pop",
        fixture: Fixture::Empty,
        method: "DELETE",
        target: Target::Literal("/missing"),
        request_headers: NO_REQUEST_HEADERS,
        body: b"",
        status: 404,
        response_headers: PLAIN_HEADER,
    },
    Probe {
        endpoint: "site copy",
        fixture: Fixture::Site,
        method: "COPY",
        target: Target::Literal("/hello"),
        request_headers: &[("destination", "/hello")],
        body: b"",
        status: 409,
        response_headers: PLAIN_HEADER,
    },
    Probe {
        endpoint: "site move",
        fixture: Fixture::Site,
        method: "MOVE",
        target: Target::Literal("/hello"),
        request_headers: NO_REQUEST_HEADERS,
        body: b"",
        status: 400,
        response_headers: PLAIN_HEADER,
    },
    Probe {
        endpoint: "site undo",
        fixture: Fixture::Empty,
        method: "UNDO",
        target: Target::Literal("/missing"),
        request_headers: NO_REQUEST_HEADERS,
        body: b"",
        status: 404,
        response_headers: PLAIN_HEADER,
    },
    Probe {
        endpoint: "site expire",
        fixture: Fixture::Site,
        method: "EXPIRE",
        target: Target::Literal("/hello"),
        request_headers: &[("expiry-mode", "relative")],
        body: b"",
        status: 400,
        response_headers: PLAIN_HEADER,
    },
    Probe {
        endpoint: "site management",
        fixture: Fixture::Site,
        method: "MANAGE",
        target: Target::Literal("/hello"),
        request_headers: NO_REQUEST_HEADERS,
        body: b"",
        status: 400,
        response_headers: PLAIN_HEADER,
    },
    Probe {
        endpoint: "site management",
        fixture: Fixture::Site,
        method: "MANAGE",
        target: Target::Literal("/hello"),
        request_headers: &[("management-action", "claim")],
        body: b"",
        status: 403,
        response_headers: PLAIN_HEADER,
    },
    Probe {
        endpoint: "site file",
        fixture: Fixture::Site,
        method: "GET",
        target: Target::Literal("/hello/assets/app.js"),
        request_headers: &[("range", "bytes=99-100")],
        body: b"",
        status: 416,
        response_headers: &[
            header("content-range"),
            header("accept-ranges"),
            header("etag"),
            header("cache-control"),
        ],
    },
    Probe {
        endpoint: "file put",
        fixture: Fixture::Site,
        method: "PUT",
        target: Target::Literal("/hello/new.txt"),
        request_headers: NO_REQUEST_HEADERS,
        body: b"",
        status: 400,
        response_headers: PLAIN_HEADER,
    },
    Probe {
        endpoint: "file delete",
        fixture: Fixture::Site,
        method: "DELETE",
        target: Target::Literal("/hello/missing.txt"),
        request_headers: NO_REQUEST_HEADERS,
        body: b"",
        status: 404,
        response_headers: PLAIN_HEADER,
    },
    Probe {
        endpoint: "file expire",
        fixture: Fixture::Site,
        method: "EXPIRE",
        target: Target::Literal("/hello/missing.txt"),
        request_headers: NO_REQUEST_HEADERS,
        body: b"",
        status: 404,
        response_headers: PLAIN_HEADER,
    },
    Probe {
        endpoint: "archive get",
        fixture: Fixture::Empty,
        method: "GET",
        target: Target::Literal("/missing.tar.gz"),
        request_headers: NO_REQUEST_HEADERS,
        body: b"",
        status: 404,
        response_headers: PLAIN_HEADER,
    },
    Probe {
        endpoint: "archive pop",
        fixture: Fixture::Empty,
        method: "DELETE",
        target: Target::Literal("/missing.tar.gz"),
        request_headers: NO_REQUEST_HEADERS,
        body: b"",
        status: 404,
        response_headers: PLAIN_HEADER,
    },
    Probe {
        endpoint: "files inventory",
        fixture: Fixture::Empty,
        method: "GET",
        target: Target::Literal("/missing/FILES"),
        request_headers: &[("accept", "application/json")],
        body: b"",
        status: 404,
        response_headers: PLAIN_HEADER,
    },
    Probe {
        endpoint: "files subtree",
        fixture: Fixture::Empty,
        method: "GET",
        target: Target::Literal("/missing/FILES/path"),
        request_headers: NO_REQUEST_HEADERS,
        body: b"",
        status: 404,
        response_headers: PLAIN_HEADER,
    },
    Probe {
        endpoint: "file hash",
        fixture: Fixture::Site,
        method: "GET",
        target: Target::Literal("/hello/missing.txt/HASH"),
        request_headers: NO_REQUEST_HEADERS,
        body: b"",
        status: 404,
        response_headers: PLAIN_HEADER,
    },
    Probe {
        endpoint: "expiry inventory",
        fixture: Fixture::Empty,
        method: "GET",
        target: Target::Literal("/missing/EXPIRES"),
        request_headers: NO_REQUEST_HEADERS,
        body: b"",
        status: 404,
        response_headers: PLAIN_HEADER,
    },
    Probe {
        endpoint: "expiry target",
        fixture: Fixture::Site,
        method: "GET",
        target: Target::Literal("/hello/missing.txt/EXPIRES"),
        request_headers: NO_REQUEST_HEADERS,
        body: b"",
        status: 404,
        response_headers: PLAIN_HEADER,
    },
    Probe {
        endpoint: "immutable blob",
        fixture: Fixture::Site,
        method: "GET",
        target: Target::Literal("/.blob/hello/deadbeef"),
        request_headers: NO_REQUEST_HEADERS,
        body: b"",
        status: 404,
        response_headers: PLAIN_HEADER,
    },
    Probe {
        endpoint: "site put",
        fixture: Fixture::Managed,
        method: "PUT",
        target: Target::Literal("/hello"),
        request_headers: NO_REQUEST_HEADERS,
        body: b"<h1>unauthorized</h1>",
        status: 401,
        response_headers: &[
            header_value("content-type", "text/plain; charset=utf-8"),
            header_value("www-authenticate", "Bearer realm=\"symbol\""),
            header_value("cache-control", "no-store"),
        ],
    },
    Probe {
        endpoint: "site pop",
        fixture: Fixture::Managed,
        method: "DELETE",
        target: Target::Literal("/hello"),
        request_headers: NO_REQUEST_HEADERS,
        body: b"",
        status: 401,
        response_headers: &[
            header_value("www-authenticate", "Bearer realm=\"symbol\""),
            header_value("cache-control", "no-store"),
        ],
    },
    Probe {
        endpoint: "site move",
        fixture: Fixture::Managed,
        method: "MOVE",
        target: Target::Literal("/hello"),
        request_headers: &[("destination", "/moved")],
        body: b"",
        status: 401,
        response_headers: &[header_value("www-authenticate", "Bearer realm=\"symbol\"")],
    },
    Probe {
        endpoint: "site undo",
        fixture: Fixture::Managed,
        method: "UNDO",
        target: Target::Literal("/hello"),
        request_headers: NO_REQUEST_HEADERS,
        body: b"",
        status: 401,
        response_headers: &[header_value("www-authenticate", "Bearer realm=\"symbol\"")],
    },
    Probe {
        endpoint: "site expire",
        fixture: Fixture::Managed,
        method: "EXPIRE",
        target: Target::Literal("/hello"),
        request_headers: NO_REQUEST_HEADERS,
        body: b"",
        status: 401,
        response_headers: &[header_value("www-authenticate", "Bearer realm=\"symbol\"")],
    },
    Probe {
        endpoint: "file put",
        fixture: Fixture::Managed,
        method: "PUT",
        target: Target::Literal("/hello/new.txt"),
        request_headers: NO_REQUEST_HEADERS,
        body: b"unauthorized",
        status: 401,
        response_headers: &[header_value("www-authenticate", "Bearer realm=\"symbol\"")],
    },
    Probe {
        endpoint: "file delete",
        fixture: Fixture::Managed,
        method: "DELETE",
        target: Target::Literal("/hello/assets/app.js"),
        request_headers: NO_REQUEST_HEADERS,
        body: b"",
        status: 401,
        response_headers: &[header_value("www-authenticate", "Bearer realm=\"symbol\"")],
    },
    Probe {
        endpoint: "file expire",
        fixture: Fixture::Managed,
        method: "EXPIRE",
        target: Target::Literal("/hello/assets/app.js"),
        request_headers: NO_REQUEST_HEADERS,
        body: b"",
        status: 401,
        response_headers: &[header_value("www-authenticate", "Bearer realm=\"symbol\"")],
    },
    Probe {
        endpoint: "archive pop",
        fixture: Fixture::Managed,
        method: "DELETE",
        target: Target::Literal("/hello.tar.gz"),
        request_headers: NO_REQUEST_HEADERS,
        body: b"",
        status: 401,
        response_headers: &[header_value("www-authenticate", "Bearer realm=\"symbol\"")],
    },
];

fn endpoint(name: &str) -> &'static contract::EndpointContract {
    contract::ENDPOINTS
        .iter()
        .find(|endpoint| endpoint.name == name)
        .unwrap_or_else(|| panic!("missing typed contract endpoint {name}"))
}

async fn fixture(kind: Fixture) -> (tempfile::TempDir, Store, Router) {
    let root = tempfile::tempdir().unwrap();
    let store = Store::new(root.path().to_path_buf()).unwrap();
    if matches!(kind, Fixture::Site | Fixture::Undo | Fixture::Managed) {
        store
            .put_file("hello", "index.html", b"<h1>hello</h1>")
            .unwrap();
        store
            .put_file("hello", "assets/app.js", b"console.log('hello')")
            .unwrap();
    }
    if matches!(kind, Fixture::Managed) {
        store.operator_claim("hello").unwrap();
    }
    let app = router(App::new(store.clone()));
    if matches!(kind, Fixture::Undo) {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/hello/undo.txt")
                    .body(Body::from("undo me"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }
    (root, store, app)
}

fn target_uri(target: Target, store: &Store) -> String {
    match target {
        Target::Literal(path) => path.to_string(),
        Target::Blob => {
            let store::Node::File { hash, .. } = store.lookup("hello", "assets/app.js").unwrap()
            else {
                panic!("fixture file must be a blob");
            };
            format!("/.blob/hello/{hash}")
        }
    }
}

async fn execute(probe: Probe) {
    let (_root, store, app) = fixture(probe.fixture).await;
    let contract = endpoint(probe.endpoint);
    assert_eq!(probe.method, contract.method, "{} method", probe.endpoint);
    let mut request = Request::builder()
        .method(probe.method)
        .uri(target_uri(probe.target, &store));
    for (name, value) in probe.request_headers {
        request = request.header(*name, *value);
    }
    let response = app
        .oneshot(request.body(Body::from(probe.body)).unwrap())
        .await
        .unwrap();
    assert_eq!(
        response.status().as_u16(),
        probe.status,
        "{} response status",
        probe.endpoint
    );
    let declared = contract.success_statuses.contains(&probe.status)
        || contract.error_statuses.contains(&probe.status);
    assert!(
        declared,
        "{} observed undeclared status {}",
        probe.endpoint, probe.status
    );
    for expected in probe.response_headers {
        assert!(
            contract
                .response_headers
                .iter()
                .any(|name| name.eq_ignore_ascii_case(expected.name))
                || matches!(
                    expected.name,
                    "content-type"
                        | "cache-control"
                        | "www-authenticate"
                        | "content-range"
                        | "accept-ranges"
                ),
            "{} test expects undocumented response header {}",
            probe.endpoint,
            expected.name
        );
        let observed = response.headers().get(expected.name).unwrap_or_else(|| {
            panic!(
                "{} status {} omitted expected response header {}",
                probe.endpoint, probe.status, expected.name
            )
        });
        if let Some(value) = expected.value {
            assert_eq!(
                observed, value,
                "{} {} header",
                probe.endpoint, expected.name
            );
        }
    }
}

async fn conditional_probe(endpoint_name: &str, path: &str) {
    let (_root, _store, app) = fixture(Fixture::Site).await;
    let first = app
        .clone()
        .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let etag = first.headers()["etag"].clone();
    let response = app
        .oneshot(
            Request::builder()
                .uri(path)
                .header("if-none-match", etag)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::NOT_MODIFIED,
        "{endpoint_name}"
    );
    let contract = endpoint(endpoint_name);
    assert!(contract.success_statuses.contains(&304));
    assert!(response.headers().contains_key("etag"));
    assert!(response.headers().contains_key("cache-control"));
}

#[tokio::test]
async fn every_contract_endpoint_executes_success_and_normative_error_probes() {
    assert_eq!(SUCCESS_PROBES.len(), contract::ENDPOINTS.len());
    for endpoint in contract::ENDPOINTS {
        assert_eq!(
            SUCCESS_PROBES
                .iter()
                .filter(|probe| probe.endpoint == endpoint.name)
                .count(),
            1,
            "{} must have exactly one primary success probe",
            endpoint.name
        );
    }
    for probe in SUCCESS_PROBES.iter().chain(ERROR_PROBES) {
        execute(*probe).await;
    }
}

#[tokio::test]
async fn cacheable_contract_endpoints_execute_conditional_requests() {
    for (name, path) in [
        ("docs", "/"),
        ("installer", "/install.sh"),
        ("client", "/symbol.sh"),
        ("site listing", "/FILES"),
        ("site index", "/hello/"),
        ("files subtree", "/hello/FILES/assets/"),
    ] {
        conditional_probe(name, path).await;
    }
}

#[tokio::test]
async fn mutation_contract_executes_noop_and_stale_write_paths() {
    let (_root, _store, app) = fixture(Fixture::Site).await;
    let no_op = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/hello")
                .header("content-type", "text/html")
                .body(Body::from("<h1>hello</h1>"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(no_op.status(), StatusCode::OK);
    assert!(endpoint("site put").success_statuses.contains(&200));
    assert!(!no_op.headers().contains_key("undo-token"));

    let inventory = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/hello/FILES")
                .header("accept", "application/json")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let stale_etag = inventory.headers()["etag"].clone();
    let update = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/hello/first.txt")
                .header("if-match", stale_etag.clone())
                .body(Body::from("first"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(update.status(), StatusCode::OK);

    let stale = app
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/hello/raced.txt")
                .header("if-match", stale_etag)
                .body(Body::from("must not commit"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(stale.status(), StatusCode::PRECONDITION_FAILED);
    assert!(endpoint("file put").error_statuses.contains(&412));
    assert!(stale.headers().contains_key("etag"));
    assert!(stale.headers().contains_key("content-revision"));
}
