use super::*;
use axum::body::{Body, to_bytes};
use axum::http::{HeaderMap, Request};
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
        endpoint: "api documentation",
        fixture: Fixture::Empty,
        method: "GET",
        target: Target::Literal("/API/JS"),
        request_headers: &[("accept", "text/markdown")],
        body: b"",
        status: 200,
        response_headers: &[
            header_value("content-type", "text/markdown; charset=utf-8"),
            header("etag"),
            header_value("cache-control", "no-cache"),
            header_value("vary", "Accept, User-Agent"),
            header_value("link", "</API/JS>; rel=\"canonical\""),
        ],
    },
    Probe {
        endpoint: "api client asset",
        fixture: Fixture::Empty,
        method: "GET",
        target: Target::Literal("/symbol.js"),
        request_headers: NO_REQUEST_HEADERS,
        body: b"",
        status: 200,
        response_headers: &[
            header_value("content-type", "text/javascript; charset=utf-8"),
            header("etag"),
            header_value("cache-control", "no-cache"),
        ],
    },
    Probe {
        endpoint: "api client hash",
        fixture: Fixture::Empty,
        method: "GET",
        target: Target::Literal("/symbol.js/HASH"),
        request_headers: NO_REQUEST_HEADERS,
        body: b"",
        status: 200,
        response_headers: PLAIN_HEADER,
    },
    Probe {
        endpoint: "api version",
        fixture: Fixture::Empty,
        method: "GET",
        target: Target::Literal("/API/VERSION"),
        request_headers: NO_REQUEST_HEADERS,
        body: b"",
        status: 200,
        response_headers: &[
            header_value("content-type", "application/json; charset=utf-8"),
            header("etag"),
            header_value("cache-control", "no-cache"),
        ],
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
    Probe {
        endpoint: "alias batch",
        fixture: Fixture::Site,
        method: "ALIAS",
        target: Target::Literal("/hello/"),
        request_headers: &[("content-type", "application/json")],
        body: br#"{"aliases":[{"path":"batch-link","target":"assets/app.js"}]}"#,
        status: 201,
        response_headers: MUTATION_HEADERS,
    },
    Probe {
        endpoint: "alias file",
        fixture: Fixture::Site,
        method: "ALIAS",
        target: Target::Literal("/hello/latest.js"),
        request_headers: &[("alias-target", "assets/app.js")],
        body: b"",
        status: 201,
        response_headers: MUTATION_HEADERS,
    },
    Probe {
        endpoint: "allocated file",
        fixture: Fixture::Site,
        method: "POST",
        target: Target::Literal("/hello/generated/"),
        request_headers: &[
            ("content-type", "application/octet-stream"),
            ("file-extension", "bin"),
        ],
        body: b"allocated",
        status: 201,
        response_headers: &[
            header("content-type"),
            header("content-location"),
            header("location"),
            header("etag"),
            header("content-revision"),
            header("undo-token"),
            header("undo-expires"),
        ],
    },
    Probe {
        endpoint: "file replace",
        fixture: Fixture::Site,
        method: "REPLACE",
        target: Target::Literal("/hello/assets/app.js"),
        request_headers: &[("if-content-match", "{fixture-file-hash}")],
        body: b"replacement",
        status: 200,
        response_headers: MUTATION_HEADERS,
    },
    Probe {
        endpoint: "file splice",
        fixture: Fixture::Site,
        method: "PATCH",
        target: Target::Literal("/hello/assets/app.js"),
        request_headers: &[
            ("if-content-match", "{fixture-file-hash}"),
            ("splice", "offset=0; delete=0; insert=1"),
        ],
        body: b"X",
        status: 200,
        response_headers: MUTATION_HEADERS,
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
    Probe {
        endpoint: "alias batch",
        fixture: Fixture::Site,
        method: "ALIAS",
        target: Target::Literal("/hello/"),
        request_headers: &[("content-type", "application/json")],
        body: br#"{"aliases":[]}"#,
        status: 400,
        response_headers: PLAIN_HEADER,
    },
    Probe {
        endpoint: "alias file",
        fixture: Fixture::Site,
        method: "ALIAS",
        target: Target::Literal("/hello/latest.js"),
        request_headers: NO_REQUEST_HEADERS,
        body: b"",
        status: 400,
        response_headers: PLAIN_HEADER,
    },
    Probe {
        endpoint: "allocated file",
        fixture: Fixture::Empty,
        method: "POST",
        target: Target::Literal("/missing/"),
        request_headers: NO_REQUEST_HEADERS,
        body: b"must not spool",
        status: 404,
        response_headers: PLAIN_HEADER,
    },
    Probe {
        endpoint: "file replace",
        fixture: Fixture::Site,
        method: "REPLACE",
        target: Target::Literal("/hello/assets/app.js"),
        request_headers: NO_REQUEST_HEADERS,
        body: b"replacement",
        status: 400,
        response_headers: PLAIN_HEADER,
    },
    Probe {
        endpoint: "file splice",
        fixture: Fixture::Site,
        method: "PATCH",
        target: Target::Literal("/hello/assets/app.js"),
        request_headers: &[
            ("if-content-match", "{fixture-file-hash}"),
            ("splice", "offset=999; delete=0; insert=0"),
        ],
        body: b"",
        status: 416,
        response_headers: PLAIN_HEADER,
    },
    Probe {
        endpoint: "alias batch",
        fixture: Fixture::Managed,
        method: "ALIAS",
        target: Target::Literal("/hello/"),
        request_headers: &[("content-type", "application/json")],
        body: br#"{"aliases":[{"path":"blocked","target":"index.html"}]}"#,
        status: 401,
        response_headers: &[header_value("www-authenticate", "Bearer realm=\"symbol\"")],
    },
    Probe {
        endpoint: "alias file",
        fixture: Fixture::Managed,
        method: "ALIAS",
        target: Target::Literal("/hello/blocked"),
        request_headers: &[("alias-target", "index.html")],
        body: b"",
        status: 401,
        response_headers: &[header_value("www-authenticate", "Bearer realm=\"symbol\"")],
    },
    Probe {
        endpoint: "allocated file",
        fixture: Fixture::Managed,
        method: "POST",
        target: Target::Literal("/hello/"),
        request_headers: NO_REQUEST_HEADERS,
        body: b"must not spool",
        status: 401,
        response_headers: &[header_value("www-authenticate", "Bearer realm=\"symbol\"")],
    },
    Probe {
        endpoint: "file replace",
        fixture: Fixture::Managed,
        method: "REPLACE",
        target: Target::Literal("/hello/index.html"),
        request_headers: &[("if-content-match", "{fixture-file-hash}")],
        body: b"must not spool",
        status: 401,
        response_headers: &[header_value("www-authenticate", "Bearer realm=\"symbol\"")],
    },
    Probe {
        endpoint: "file splice",
        fixture: Fixture::Managed,
        method: "PATCH",
        target: Target::Literal("/hello/index.html"),
        request_headers: &[
            ("if-content-match", "{fixture-file-hash}"),
            ("splice", "offset=0; delete=0; insert=1"),
        ],
        body: b"x",
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
            format!("/.blob/hello/{}", hash.to_hex())
        }
    }
}

async fn execute(probe: Probe) {
    let (_root, store, app) = fixture(probe.fixture).await;
    let contract = endpoint(probe.endpoint);
    assert_eq!(probe.method, contract.method, "{} method", probe.endpoint);
    let fixture_file_hash = match store.lookup("hello", "assets/app.js") {
        Ok(store::Node::File { hash, .. }) => Some(hash.to_wire()),
        Ok(store::Node::Dir) | Err(_) => None,
    };
    let mut request = Request::builder()
        .method(probe.method)
        .uri(target_uri(probe.target, &store));
    for (name, value) in probe.request_headers {
        request = request.header(
            *name,
            if *value == "{fixture-file-hash}" {
                fixture_file_hash
                    .as_deref()
                    .expect("probe fixture must include assets/app.js")
            } else {
                value
            },
        );
    }
    let response = app
        .clone()
        .oneshot(request.body(Body::from(probe.body)).unwrap())
        .await
        .unwrap();
    assert_eq!(
        response.status().as_u16(),
        probe.status,
        "{} response status",
        probe.endpoint
    );
    let declared = contract.outcome(probe.status, probe_variant(probe));
    assert!(
        declared.is_some(),
        "{} observed undeclared status {} variant {:?}",
        probe.endpoint,
        probe.status,
        probe_variant(probe)
    );
    for expected in probe.response_headers {
        assert!(
            contract
                .response_headers()
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
    let location = response
        .headers()
        .get(header::LOCATION)
        .map(|location| location.to_str().unwrap().to_string());
    assert_success_location_is_readable(probe, &app, location).await;
    assert_exact_outcome(probe, response).await;
}

async fn assert_success_location_is_readable(probe: Probe, app: &Router, location: Option<String>) {
    let endpoint = endpoint(probe.endpoint);
    if endpoint
        .exact_outcome(
            contract::OutcomeKind::Success,
            probe.status,
            probe_variant(probe),
        )
        .is_none()
    {
        return;
    }
    let Some(location) = location else {
        return;
    };
    let location = location.parse::<axum::http::Uri>().unwrap();
    let target = location
        .path_and_query()
        .map_or_else(|| location.path(), axum::http::uri::PathAndQuery::as_str);
    let fetched = app
        .clone()
        .oneshot(Request::builder().uri(target).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert!(
        fetched.status().is_success() || fetched.status().is_redirection(),
        "{} returned unreadable Location {target}: {}",
        probe.endpoint,
        fetched.status()
    );
}

async fn assert_exact_outcome(probe: Probe, response: Response) {
    let expected = endpoint(probe.endpoint)
        .outcome(probe.status, probe_variant(probe))
        .unwrap_or_else(|| {
            panic!(
                "{} status {} variant {:?} lacks an exact outcome",
                probe.endpoint,
                probe.status,
                probe_variant(probe)
            )
        });
    for required in expected.required_headers {
        assert!(
            response.headers().contains_key(*required),
            "{} status {} omitted exact-outcome header {}",
            probe.endpoint,
            probe.status,
            required
        );
    }
    for forbidden in expected.forbidden_headers {
        assert!(
            !response.headers().contains_key(*forbidden),
            "{} status {} included forbidden exact-outcome header {}",
            probe.endpoint,
            probe.status,
            forbidden
        );
    }
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_string();
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    match expected.body {
        contract::WireBody::Empty => assert!(body.is_empty()),
        contract::WireBody::Json => {
            assert!(content_type.starts_with("application/json"));
            serde_json::from_slice::<serde_json::Value>(&body).unwrap();
        }
        contract::WireBody::PlainText => {
            assert!(
                content_type.starts_with("text/"),
                "{} status {} returned {content_type}",
                probe.endpoint,
                probe.status
            );
            std::str::from_utf8(&body).unwrap();
        }
        contract::WireBody::Binary => assert!(!body.is_empty()),
    }
}

fn probe_variant(probe: Probe) -> contract::RequestVariant {
    if probe.endpoint != "allocated file" {
        return contract::RequestVariant::Default;
    }
    match probe
        .request_headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("allocation-action"))
        .map_or("create", |(_, value)| *value)
    {
        "create" => contract::RequestVariant::Create,
        "finalize" => contract::RequestVariant::Finalize,
        "propose" => contract::RequestVariant::Propose,
        "cancel" => contract::RequestVariant::Cancel,
        variant => panic!("unknown allocation probe variant {variant}"),
    }
}

async fn send(
    app: &Router,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: impl Into<Body>,
) -> Response {
    let mut request = Request::builder().method(method).uri(path);
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    app.clone()
        .oneshot(request.body(body.into()).unwrap())
        .await
        .unwrap()
}

async fn json(response: Response) -> (HeaderMap, serde_json::Value) {
    let headers = response.headers().clone();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (
        headers,
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "expected JSON response ({error}): {}",
                String::from_utf8_lossy(&bytes)
            )
        }),
    )
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
    assert!(
        contract
            .exact_outcome(
                contract::OutcomeKind::Success,
                304,
                contract::RequestVariant::Default,
            )
            .is_some()
    );
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
async fn allocated_post_rejects_reserved_control_directories_consistently() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::new(root.path().to_path_buf()).unwrap();
    store.put_file("hello", "index.html", b"site").unwrap();
    let token = store.operator_claim("hello").unwrap().encode();
    let authorization = format!("Bearer {token}");
    let reserved_body = format!("{}\n", contract::RESERVED_MUTATION_ERROR);
    let app = router(App::new(store.clone()));
    for path in [
        "/hello/FILES",
        "/hello/FILES/",
        "/hello/FILES/generated",
        "/hello/FILES/generated/",
        "/hello/UNDO",
        "/hello/UNDO/",
        "/hello/UNDO/x",
        "/hello/UNDO/x/",
        "/hello/EXPIRES",
        "/hello/EXPIRES/",
        "/hello/EXPIRES/x",
        "/hello/EXPIRES/x/",
        "/hello/safe/FILES",
        "/hello/safe/HASH",
        "/hello/safe/UNDO",
        "/hello/safe/EXPIRES",
    ] {
        for method in [
            "POST", "ALIAS", "REPLACE", "PATCH", "PUT", "DELETE", "EXPIRE",
        ] {
            let unauthorized = send(&app, method, path, &[], "must not spool").await;
            assert_eq!(
                unauthorized.status(),
                StatusCode::UNAUTHORIZED,
                "{method} {path} must authenticate first"
            );
            assert_eq!(
                to_bytes(unauthorized.into_body(), usize::MAX)
                    .await
                    .unwrap(),
                "error: management token required\n",
                "{method} {path} unauthorized body"
            );
            let rejected = send(
                &app,
                method,
                path,
                &[("authorization", &authorization)],
                "must not spool",
            )
            .await;
            assert_eq!(
                rejected.status(),
                StatusCode::BAD_REQUEST,
                "{method} {path}"
            );
            assert_eq!(
                to_bytes(rejected.into_body(), usize::MAX).await.unwrap(),
                reserved_body,
                "{method} {path} reserved-path body"
            );
        }
    }
    for path in [
        "FILES/generated",
        "UNDO/x",
        "EXPIRES/x",
        "safe/FILES",
        "safe/HASH",
        "safe/UNDO",
        "safe/EXPIRES",
    ] {
        assert!(matches!(
            store.lookup("hello", path),
            Err(StoreError::NotFound)
        ));
    }
    assert_eq!(
        std::fs::read_dir(root.path().join("tmp")).unwrap().count(),
        0,
        "virtual namespace rejection must precede body spooling"
    );
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
    assert!(
        endpoint("site put")
            .exact_outcome(
                contract::OutcomeKind::Success,
                200,
                contract::RequestVariant::Default,
            )
            .is_some()
    );
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
    assert!(
        endpoint("file put")
            .exact_outcome(
                contract::OutcomeKind::Error,
                412,
                contract::RequestVariant::Default,
            )
            .is_some()
    );
    assert!(stale.headers().contains_key("etag"));
    assert!(stale.headers().contains_key("content-revision"));
}

#[tokio::test]
#[expect(clippy::too_many_lines)]
async fn phase_five_mutations_cover_replay_conflict_limits_and_two_phase_outcomes() {
    let (_root, store, app) = fixture(Fixture::Site).await;

    let allocated = send(
        &app,
        "POST",
        "/hello/generated/",
        &[
            ("content-type", "application/octet-stream"),
            ("file-prefix", "asset-"),
            ("file-extension", ".BIN"),
            ("idempotency-key", "allocate-dropped"),
        ],
        "allocated-body",
    )
    .await;
    assert_eq!(allocated.status(), StatusCode::CREATED);
    let (allocated_headers, allocated_json) = json(allocated).await;
    assert!(allocated_headers.contains_key("content-location"));
    assert_eq!(allocated_json["outcome"], "created");
    assert_eq!(allocated_json["naming"]["extension"], "bin");
    let allocated_path = allocated_json["path"].as_str().unwrap().to_string();
    let allocated_hash = allocated_json["hash"]
        .as_str()
        .unwrap()
        .trim_start_matches("blake3:")
        .to_string();

    let replay = send(
        &app,
        "POST",
        "/hello/generated/",
        &[
            ("content-type", "application/octet-stream"),
            ("file-prefix", "asset-"),
            ("file-extension", "bin"),
            ("idempotency-key", "allocate-dropped"),
        ],
        "allocated-body",
    )
    .await;
    assert_eq!(replay.status(), StatusCode::CREATED);
    assert_eq!(replay.headers()["idempotency-replayed"], "true");
    let (_, replay_json) = json(replay).await;
    assert_eq!(replay_json["path"], allocated_path);
    assert_eq!(replay_json["replayed"], true);

    let no_op = send(
        &app,
        "POST",
        "/hello/generated/",
        &[
            ("content-type", "application/octet-stream"),
            ("file-prefix", "asset-"),
            ("file-extension", "bin"),
            ("idempotency-key", "allocate-noop"),
        ],
        "allocated-body",
    )
    .await;
    assert_eq!(no_op.status(), StatusCode::OK);
    assert!(!no_op.headers().contains_key("undo-token"));
    let (_, no_op_json) = json(no_op).await;
    assert_eq!(no_op_json["outcome"], "existing");
    assert_eq!(no_op_json["changed"], false);

    let conflict = send(
        &app,
        "POST",
        "/hello/generated/",
        &[
            ("content-type", "application/octet-stream"),
            ("file-prefix", "asset-"),
            ("file-extension", "bin"),
            ("idempotency-key", "allocate-dropped"),
        ],
        "different",
    )
    .await;
    assert_eq!(conflict.status(), StatusCode::CONFLICT);

    let expiring = send(
        &app,
        "POST",
        "/hello/generated/",
        &[
            ("content-type", "application/octet-stream"),
            ("file-prefix", "asset-"),
            ("file-extension", "bin"),
            ("expiry-mode", "relative"),
            ("expiry-in", "1h"),
            ("idempotency-key", "allocate-expiry"),
        ],
        "allocated-body",
    )
    .await;
    assert_eq!(expiring.status(), StatusCode::OK);
    assert_eq!(
        store
            .expiry_report("hello", &allocated_path)
            .unwrap()
            .own_policy
            .unwrap()
            .mode,
        expiry::ExpiryMode::Relative
    );
    let allocated_route = format!("/hello/{allocated_path}");
    let relocated = send(
        &app,
        "REPLACE",
        &allocated_route,
        &[
            ("if-content-match", &allocated_hash),
            ("idempotency-key", "allocated-replace-dropped"),
        ],
        "relocated allocated body",
    )
    .await;
    assert_eq!(relocated.status(), StatusCode::OK);
    let (_, relocated_json) = json(relocated).await;
    assert_eq!(relocated_json["outcome"], "relocated");
    assert_eq!(relocated_json["relocated"], true);
    let relocated_path = relocated_json["new_path"].as_str().unwrap().to_string();
    assert_ne!(relocated_path, allocated_path);
    assert!(matches!(
        store.lookup("hello", &allocated_path),
        Err(StoreError::NotFound)
    ));
    let relocated_replay = send(
        &app,
        "REPLACE",
        &allocated_route,
        &[
            ("if-content-match", &allocated_hash),
            ("idempotency-key", "allocated-replace-dropped"),
        ],
        "relocated allocated body",
    )
    .await;
    assert_eq!(relocated_replay.status(), StatusCode::OK);
    assert_eq!(relocated_replay.headers()["idempotency-replayed"], "true");
    assert_eq!(
        store
            .expiry_report("hello", &relocated_path)
            .unwrap()
            .own_policy
            .unwrap()
            .mode,
        expiry::ExpiryMode::Relative
    );

    let alias = send(
        &app,
        "ALIAS",
        "/hello/latest.js",
        &[
            ("alias-target", "assets/app.js"),
            ("idempotency-key", "alias-dropped"),
        ],
        Body::empty(),
    )
    .await;
    assert_eq!(alias.status(), StatusCode::CREATED);
    let alias_body = to_bytes(alias.into_body(), usize::MAX).await.unwrap();
    let alias_replay = send(
        &app,
        "ALIAS",
        "/hello/latest.js",
        &[
            ("alias-target", "assets/app.js"),
            ("idempotency-key", "alias-dropped"),
        ],
        Body::empty(),
    )
    .await;
    assert_eq!(alias_replay.status(), StatusCode::CREATED);
    assert_eq!(alias_replay.headers()["idempotency-replayed"], "true");
    let replay_drift = send(
        &app,
        "ALIAS",
        "/hello/latest.js",
        &[
            ("alias-target", "index.html"),
            ("idempotency-key", "alias-retarget"),
        ],
        Body::empty(),
    )
    .await;
    assert_eq!(replay_drift.status(), StatusCode::OK);
    let replay_after_retarget = send(
        &app,
        "ALIAS",
        "/hello/latest.js",
        &[
            ("alias-target", "assets/app.js"),
            ("idempotency-key", "alias-dropped"),
        ],
        Body::empty(),
    )
    .await;
    let replay_after_retarget = to_bytes(replay_after_retarget.into_body(), usize::MAX)
        .await
        .unwrap();
    let mut original_alias: serde_json::Value = serde_json::from_slice(&alias_body).unwrap();
    let replay_after_retarget: serde_json::Value =
        serde_json::from_slice(&replay_after_retarget).unwrap();
    original_alias["replayed"] = serde_json::Value::Bool(true);
    assert_eq!(
        replay_after_retarget, original_alias,
        "only the replay marker may differ from the stored alias receipt"
    );
    assert_eq!(
        replay_after_retarget["target"], "assets/app.js",
        "alias replay must use its stored receipt rather than current state"
    );
    let changed_tree_guard = send(
        &app,
        "ALIAS",
        "/hello/latest.js",
        &[
            ("alias-target", "assets/app.js"),
            ("idempotency-key", "alias-dropped"),
            (
                "if-match",
                "blake3:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            ),
        ],
        Body::empty(),
    )
    .await;
    assert_eq!(changed_tree_guard.status(), StatusCode::CONFLICT);
    let alias_noop = send(
        &app,
        "ALIAS",
        "/hello/latest.js",
        &[
            ("alias-target", "index.html"),
            ("idempotency-key", "alias-noop"),
        ],
        Body::empty(),
    )
    .await;
    assert_eq!(alias_noop.status(), StatusCode::OK);
    assert!(!alias_noop.headers().contains_key("undo-token"));

    let batch = send(
        &app,
        "ALIAS",
        "/hello/",
        &[
            ("content-type", "application/json"),
            ("idempotency-key", "alias-batch"),
        ],
        r#"{"aliases":[{"path":"home","target":"index.html"},{"path":"app","target":"assets/app.js"}]}"#,
    )
    .await;
    assert_eq!(batch.status(), StatusCode::CREATED);
    let mixed_batch = send(
        &app,
        "ALIAS",
        "/hello/",
        &[("content-type", "application/json")],
        r#"{"aliases":[{"path":"home","target":"assets/app.js"},{"path":"new-link","target":"index.html"}]}"#,
    )
    .await;
    assert_eq!(
        mixed_batch.status(),
        StatusCode::OK,
        "a create+retarget batch is not wholly created"
    );
    let inventory = send(
        &app,
        "GET",
        "/hello/FILES",
        &[("accept", "application/json")],
        Body::empty(),
    )
    .await;
    let (_, inventory) = json(inventory).await;
    assert_eq!(inventory["aliases"].as_array().unwrap().len(), 4);
    let listing = send(
        &app,
        "GET",
        "/hello/FILES",
        &[("accept", "text/plain")],
        Body::empty(),
    )
    .await;
    let listing = to_bytes(listing.into_body(), usize::MAX).await.unwrap();
    assert!(String::from_utf8_lossy(&listing).contains("latest.js -> index.html"));
    let stats = send(&app, "GET", "/STATS", &[], Body::empty()).await;
    let (_, stats) = json(stats).await;
    assert_eq!(stats["aliases"], 4);

    let store::Node::File {
        hash: original_hash,
        ..
    } = store.lookup("hello", "assets/app.js").unwrap()
    else {
        panic!("fixture path is a file");
    };
    let replaced = send(
        &app,
        "REPLACE",
        "/hello/assets/app.js",
        &[
            ("if-content-match", &original_hash.to_wire()),
            ("idempotency-key", "replace-dropped"),
        ],
        "replacement",
    )
    .await;
    assert_eq!(replaced.status(), StatusCode::OK);
    let (_, replaced_json) = json(replaced).await;
    let replacement_hash = replaced_json["new_hash"]
        .as_str()
        .unwrap()
        .trim_start_matches("blake3:")
        .to_string();
    let replace_replay = send(
        &app,
        "REPLACE",
        "/hello/assets/app.js",
        &[
            ("if-content-match", &original_hash.to_wire()),
            ("idempotency-key", "replace-dropped"),
        ],
        "replacement",
    )
    .await;
    assert_eq!(replace_replay.status(), StatusCode::OK);
    assert_eq!(replace_replay.headers()["idempotency-replayed"], "true");
    let stale_replace = send(
        &app,
        "REPLACE",
        "/hello/assets/app.js",
        &[
            ("if-content-match", &original_hash.to_wire()),
            ("idempotency-key", "replace-stale"),
        ],
        "stale",
    )
    .await;
    assert_eq!(stale_replace.status(), StatusCode::PRECONDITION_FAILED);
    assert!(stale_replace.headers().contains_key("content-revision"));

    let splice = send(
        &app,
        "PATCH",
        "/hello/assets/app.js",
        &[
            ("if-content-match", &replacement_hash),
            ("splice", "offset=0; delete=1; insert=1"),
            ("idempotency-key", "splice-dropped"),
        ],
        "R",
    )
    .await;
    assert_eq!(splice.status(), StatusCode::OK);
    let (_, splice_json) = json(splice).await;
    let spliced_hash = splice_json["new_hash"]
        .as_str()
        .unwrap()
        .trim_start_matches("blake3:")
        .to_string();
    let splice_replay = send(
        &app,
        "PATCH",
        "/hello/assets/app.js",
        &[
            ("if-content-match", &replacement_hash),
            ("splice", "offset=0; delete=1; insert=1"),
            ("idempotency-key", "splice-dropped"),
        ],
        "R",
    )
    .await;
    assert_eq!(splice_replay.status(), StatusCode::OK);
    assert_eq!(splice_replay.headers()["idempotency-replayed"], "true");
    let range = send(
        &app,
        "PATCH",
        "/hello/assets/app.js",
        &[
            ("if-content-match", &spliced_hash),
            ("splice", "offset=999; delete=0; insert=0"),
        ],
        Body::empty(),
    )
    .await;
    assert_eq!(range.status(), StatusCode::RANGE_NOT_SATISFIABLE);

    let mut framed = Vec::from(b"SYMSPL1\0".as_slice());
    framed.extend_from_slice(&1_u32.to_be_bytes());
    framed.extend_from_slice(&0_u32.to_be_bytes());
    framed.extend_from_slice(&0_u64.to_be_bytes());
    framed.extend_from_slice(&0_u64.to_be_bytes());
    framed.extend_from_slice(&1_u64.to_be_bytes());
    framed.push(b'F');
    let framed_response = send(
        &app,
        "PATCH",
        "/hello/assets/app.js",
        &[
            ("if-content-match", &spliced_hash),
            ("content-type", "Application/Vnd.Symbol.Splice; VERSION=1"),
            ("idempotency-key", "splice-frame"),
        ],
        framed,
    )
    .await;
    assert_eq!(framed_response.status(), StatusCode::OK);

    let descriptors = vec!["offset=0; delete=0; insert=0"; 65].join(",");
    let over_limit = send(
        &app,
        "PATCH",
        "/hello/assets/app.js",
        &[
            ("if-content-match", &spliced_hash),
            ("splice", &descriptors),
        ],
        Body::empty(),
    )
    .await;
    assert_eq!(over_limit.status(), StatusCode::PAYLOAD_TOO_LARGE);

    let proposal = send(
        &app,
        "POST",
        "/hello/custom/",
        &[
            ("allocation-action", "propose"),
            ("content-type", "text/plain"),
            ("expiry-mode", "relative"),
            ("expiry-in", "1h"),
            ("idempotency-key", "proposal-dropped"),
        ],
        "custom body",
    )
    .await;
    assert_eq!(proposal.status(), StatusCode::ACCEPTED);
    let proposal_etag = proposal.headers()[header::ETAG]
        .to_str()
        .unwrap()
        .to_string();
    let proposal_revision = proposal.headers()["content-revision"]
        .to_str()
        .unwrap()
        .to_string();
    let (_, proposal_json) = json(proposal).await;
    let proposal_token = proposal_json["allocation_token"]
        .as_str()
        .unwrap()
        .to_string();
    store
        .put_file("hello", "after-proposal.txt", b"intervening mutation")
        .unwrap();
    let proposal_replay = send(
        &app,
        "POST",
        "/hello/custom/",
        &[
            ("allocation-action", "propose"),
            ("content-type", "text/plain"),
            ("expiry-mode", "relative"),
            ("expiry-in", "1h"),
            ("idempotency-key", "proposal-dropped"),
        ],
        "custom body",
    )
    .await;
    assert_eq!(proposal_replay.status(), StatusCode::ACCEPTED);
    assert_eq!(proposal_replay.headers()["idempotency-replayed"], "true");
    assert_eq!(proposal_replay.headers()[header::ETAG], proposal_etag);
    assert_eq!(
        proposal_replay.headers()["content-revision"],
        proposal_revision
    );
    let changed_hint = send(
        &app,
        "POST",
        "/hello/custom/",
        &[
            ("allocation-action", "propose"),
            ("content-type", "text/plain"),
            ("file-extension", "bin"),
            ("expiry-mode", "relative"),
            ("expiry-in", "1h"),
            ("idempotency-key", "proposal-dropped"),
        ],
        "custom body",
    )
    .await;
    assert_eq!(changed_hint.status(), StatusCode::CONFLICT);
    let finalized = send(
        &app,
        "POST",
        "/hello/custom/",
        &[
            ("allocation-action", "finalize"),
            ("allocation-token", &proposal_token),
            ("file-name", "chosen"),
            ("idempotency-key", "finalize-dropped"),
        ],
        Body::empty(),
    )
    .await;
    assert_eq!(finalized.status(), StatusCode::CREATED);
    let custom_content = send(&app, "GET", "/hello/custom/chosen", &[], Body::empty()).await;
    assert_eq!(custom_content.status(), StatusCode::OK);
    assert_eq!(custom_content.headers()[header::CONTENT_TYPE], "text/plain");
    let custom_alias = send(
        &app,
        "ALIAS",
        "/hello/custom-link",
        &[("alias-target", "custom/chosen")],
        Body::empty(),
    )
    .await;
    assert_eq!(custom_alias.status(), StatusCode::CREATED);
    let custom_alias_content = send(&app, "GET", "/hello/custom-link", &[], Body::empty()).await;
    assert_eq!(
        custom_alias_content.headers()[header::CONTENT_TYPE],
        "text/plain"
    );
    let finalized_replay = send(
        &app,
        "POST",
        "/hello/custom/",
        &[
            ("allocation-action", "finalize"),
            ("allocation-token", &proposal_token),
            ("file-name", "chosen"),
            ("idempotency-key", "finalize-dropped"),
        ],
        Body::empty(),
    )
    .await;
    assert_eq!(finalized_replay.status(), StatusCode::CREATED);
    assert_eq!(finalized_replay.headers()["idempotency-replayed"], "true");
    assert_eq!(
        store
            .expiry_report("hello", "custom/chosen")
            .unwrap()
            .own_policy
            .unwrap()
            .mode,
        expiry::ExpiryMode::Relative
    );

    let cancellable = send(
        &app,
        "POST",
        "/hello/custom/",
        &[
            ("allocation-action", "propose"),
            ("content-type", "application/octet-stream"),
            ("idempotency-key", "proposal-cancel"),
        ],
        "cancel me",
    )
    .await;
    let (_, cancellable) = json(cancellable).await;
    let cancel_token = cancellable["allocation_token"].as_str().unwrap();
    let wrong_folder = send(
        &app,
        "POST",
        "/hello/other/",
        &[
            ("allocation-action", "cancel"),
            ("allocation-token", cancel_token),
        ],
        Body::empty(),
    )
    .await;
    assert_eq!(wrong_folder.status(), StatusCode::NOT_FOUND);
    let cancel_tree = store.site_inventory("hello").unwrap().tree_hash;
    let cancelled = send(
        &app,
        "POST",
        "/hello/custom/",
        &[
            ("allocation-action", "cancel"),
            ("allocation-token", cancel_token),
            ("idempotency-key", "cancel-dropped"),
            ("if-match", &cancel_tree),
        ],
        Body::empty(),
    )
    .await;
    assert_eq!(cancelled.status(), StatusCode::OK);
    let cancel_etag = cancelled.headers()[header::ETAG]
        .to_str()
        .unwrap()
        .to_string();
    let cancel_revision = cancelled.headers()["content-revision"]
        .to_str()
        .unwrap()
        .to_string();
    let (_, cancelled_body) = json(cancelled).await;
    store
        .put_file("hello", "after-cancel.txt", b"changed tree")
        .unwrap();
    let cancelled_replay = send(
        &app,
        "POST",
        "/hello/custom/",
        &[
            ("allocation-action", "cancel"),
            ("allocation-token", cancel_token),
            ("idempotency-key", "cancel-dropped"),
            ("if-match", &cancel_tree),
        ],
        Body::empty(),
    )
    .await;
    assert_eq!(cancelled_replay.status(), StatusCode::OK);
    assert_eq!(cancelled_replay.headers()["idempotency-replayed"], "true");
    assert_eq!(cancelled_replay.headers()[header::ETAG], cancel_etag);
    assert_eq!(
        cancelled_replay.headers()["content-revision"],
        cancel_revision
    );
    let (_, cancelled_replay_body) = json(cancelled_replay).await;
    assert_eq!(
        cancelled_body["allocation_token"],
        cancelled_replay_body["allocation_token"]
    );
    let changed_cancel_guard = send(
        &app,
        "POST",
        "/hello/custom/",
        &[
            ("allocation-action", "cancel"),
            ("allocation-token", cancel_token),
            ("idempotency-key", "cancel-dropped"),
            (
                "if-match",
                "blake3:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            ),
        ],
        Body::empty(),
    )
    .await;
    assert_eq!(changed_cancel_guard.status(), StatusCode::CONFLICT);

    let media = send(
        &app,
        "POST",
        "/hello/media/",
        &[("content-type", "image/png"), ("file-extension", "txt")],
        "not really png",
    )
    .await;
    let (_, media) = json(media).await;
    let media_path = media["path"].as_str().unwrap();
    let media_response = send(
        &app,
        "GET",
        &format!("/hello/{media_path}"),
        &[],
        Body::empty(),
    )
    .await;
    assert_eq!(media_response.headers()[header::CONTENT_TYPE], "image/png");
}

async fn fetch_absolute_location(app: &Router, location: &str) -> Response {
    let location = location.parse::<axum::http::Uri>().unwrap();
    let target = location
        .path_and_query()
        .map_or_else(|| location.path(), axum::http::uri::PathAndQuery::as_str);
    send(app, "GET", target, &[], Body::empty()).await
}

#[tokio::test]
#[expect(clippy::too_many_lines)]
async fn phase_five_mutation_urls_encode_each_stored_path_segment_and_are_fetchable() {
    const ENCODED_SEGMENT: &str = "100%25%20caf%C3%A9%20%3F%23";
    let root = tempfile::tempdir().unwrap();
    let store = Store::new(root.path().to_path_buf()).unwrap();
    let raw_folder = "paths/100% café ?#";
    store
        .put_file("encoded", "target.txt", b"alias target")
        .unwrap();
    store
        .put_file(
            "encoded",
            &format!("{raw_folder}/replace.txt"),
            b"replace before",
        )
        .unwrap();
    store
        .put_file(
            "encoded",
            &format!("{raw_folder}/splice.txt"),
            b"splice before",
        )
        .unwrap();
    let app = router(App::new(store.clone()));

    let alias = send(
        &app,
        "ALIAS",
        &format!("/encoded/paths/{ENCODED_SEGMENT}/alias.txt"),
        &[("alias-target", "../../target.txt")],
        Body::empty(),
    )
    .await;
    assert_eq!(alias.status(), StatusCode::CREATED);
    let (alias_headers, alias_json) = json(alias).await;
    let alias_location = alias_headers[header::LOCATION].to_str().unwrap();
    let expected_alias = format!("http://symbol/encoded/paths/{ENCODED_SEGMENT}/alias.txt");
    assert_eq!(alias_location, expected_alias);
    assert_eq!(alias_json["location"], expected_alias);
    let fetched = fetch_absolute_location(&app, alias_location).await;
    assert_eq!(fetched.status(), StatusCode::OK);
    assert_eq!(
        to_bytes(fetched.into_body(), usize::MAX).await.unwrap(),
        "alias target"
    );

    let allocated = send(
        &app,
        "POST",
        &format!("/encoded/paths/{ENCODED_SEGMENT}/"),
        &[("file-extension", "bin")],
        "allocated body",
    )
    .await;
    assert_eq!(allocated.status(), StatusCode::CREATED);
    let (allocated_headers, allocated_json) = json(allocated).await;
    let allocated_location = allocated_headers[header::LOCATION].to_str().unwrap();
    let expected_allocated = format!(
        "http://symbol/encoded/paths/{ENCODED_SEGMENT}/{}",
        allocated_json["name"].as_str().unwrap()
    );
    assert_eq!(allocated_location, expected_allocated);
    assert_eq!(allocated_json["location"], expected_allocated);
    assert_eq!(allocated_json["url"], expected_allocated);
    let fetched = fetch_absolute_location(&app, allocated_location).await;
    assert_eq!(fetched.status(), StatusCode::OK);
    assert_eq!(
        to_bytes(fetched.into_body(), usize::MAX).await.unwrap(),
        "allocated body"
    );

    let replace_path = format!("{raw_folder}/replace.txt");
    let store::Node::File {
        hash: replace_hash, ..
    } = store.lookup("encoded", &replace_path).unwrap()
    else {
        panic!("replacement fixture must be a file");
    };
    let replaced = send(
        &app,
        "REPLACE",
        &format!("/encoded/paths/{ENCODED_SEGMENT}/replace.txt"),
        &[("if-content-match", &replace_hash.to_wire())],
        "replace after",
    )
    .await;
    assert_eq!(replaced.status(), StatusCode::OK);
    let (replaced_headers, replaced_json) = json(replaced).await;
    let replaced_location = replaced_headers[header::LOCATION].to_str().unwrap();
    let expected_replaced = format!("http://symbol/encoded/paths/{ENCODED_SEGMENT}/replace.txt");
    assert_eq!(replaced_location, expected_replaced);
    assert_eq!(replaced_json["location"], expected_replaced);
    let fetched = fetch_absolute_location(&app, replaced_location).await;
    assert_eq!(fetched.status(), StatusCode::OK);
    assert_eq!(
        to_bytes(fetched.into_body(), usize::MAX).await.unwrap(),
        "replace after"
    );

    let splice_path = format!("{raw_folder}/splice.txt");
    let store::Node::File {
        hash: splice_hash, ..
    } = store.lookup("encoded", &splice_path).unwrap()
    else {
        panic!("splice fixture must be a file");
    };
    let spliced = send(
        &app,
        "PATCH",
        &format!("/encoded/paths/{ENCODED_SEGMENT}/splice.txt"),
        &[
            ("if-content-match", &splice_hash.to_wire()),
            ("splice", "offset=0; delete=6; insert=6"),
        ],
        "after ",
    )
    .await;
    assert_eq!(spliced.status(), StatusCode::OK);
    let (spliced_headers, spliced_json) = json(spliced).await;
    let spliced_location = spliced_headers[header::LOCATION].to_str().unwrap();
    let expected_spliced = format!("http://symbol/encoded/paths/{ENCODED_SEGMENT}/splice.txt");
    assert_eq!(spliced_location, expected_spliced);
    assert_eq!(spliced_json["location"], expected_spliced);
    let fetched = fetch_absolute_location(&app, spliced_location).await;
    assert_eq!(fetched.status(), StatusCode::OK);
    assert_eq!(
        to_bytes(fetched.into_body(), usize::MAX).await.unwrap(),
        "after  before"
    );
}

#[tokio::test]
async fn alias_batch_rejects_manifest_unsafe_paths_and_accepts_safe_punctuation() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::new(root.path().to_path_buf()).unwrap();
    store
        .put_file("alias-input", "target.txt", b"target")
        .unwrap();
    let app = router(App::new(store));
    for path in [
        "line\nbreak",
        "bell\u{7}path",
        ".DS_Store",
        "nested/Thumbs.db",
    ] {
        let body = serde_json::json!({
            "aliases": [{"path": path, "target": "target.txt"}]
        })
        .to_string();
        let rejected = send(
            &app,
            "ALIAS",
            "/alias-input/",
            &[("content-type", "application/json")],
            body,
        )
        .await;
        assert_eq!(rejected.status(), StatusCode::BAD_REQUEST, "{path:?}");
    }

    let safe_path = "quote\" ' []{}=+,;!@~$^&()#%?.txt";
    let body = serde_json::json!({
        "aliases": [{"path": safe_path, "target": "target.txt"}]
    })
    .to_string();
    let accepted = send(
        &app,
        "ALIAS",
        "/alias-input/",
        &[("content-type", "application/json")],
        body,
    )
    .await;
    assert_eq!(accepted.status(), StatusCode::CREATED);
    let (_, accepted) = json(accepted).await;
    assert_eq!(accepted["aliases"][0]["path"], safe_path);
}

#[tokio::test]
#[expect(clippy::too_many_lines)]
async fn alias_responses_inherit_target_and_intermediate_expiry_caps() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::new(root.path().to_path_buf()).unwrap();
    store
        .put_file("expiry-alias", "target.txt", b"target")
        .unwrap();
    store
        .put_file("expiry-alias", "directory/item.txt", b"item")
        .unwrap();
    store
        .put_file("expiry-alias", "links/anchor.txt", b"anchor")
        .unwrap();
    store
        .set_expiry(
            "expiry-alias",
            "target.txt",
            Some(expiry::ExpiryPolicy::Relative {
                duration_seconds: 80,
            }),
        )
        .unwrap();
    store
        .set_expiry(
            "expiry-alias",
            "directory/item.txt",
            Some(expiry::ExpiryPolicy::Relative {
                duration_seconds: 30,
            }),
        )
        .unwrap();
    store
        .put_aliases(
            "expiry-alias",
            &[
                store::AliasSpec {
                    path: "links/target-capped",
                    target: "../target.txt",
                },
                store::AliasSpec {
                    path: "links/direct",
                    target: "../target.txt",
                },
                store::AliasSpec {
                    path: "links/chain",
                    target: "direct",
                },
                store::AliasSpec {
                    path: "view",
                    target: "directory",
                },
                store::AliasSpec {
                    path: "dangling",
                    target: "missing",
                },
            ],
            store::FileMutationOptions::default(),
        )
        .unwrap();
    store
        .set_expiry(
            "expiry-alias",
            "links",
            Some(expiry::ExpiryPolicy::Relative {
                duration_seconds: 120,
            }),
        )
        .unwrap();
    store
        .set_expiry(
            "expiry-alias",
            "links/direct",
            Some(expiry::ExpiryPolicy::Relative {
                duration_seconds: 40,
            }),
        )
        .unwrap();
    store
        .set_expiry(
            "expiry-alias",
            "links/chain",
            Some(expiry::ExpiryPolicy::Relative {
                duration_seconds: 60,
            }),
        )
        .unwrap();
    store
        .set_expiry(
            "expiry-alias",
            "dangling",
            Some(expiry::ExpiryPolicy::Relative {
                duration_seconds: 45,
            }),
        )
        .unwrap();
    let app = router(App::new(store));

    let target = send(&app, "GET", "/expiry-alias/target.txt", &[], Body::empty()).await;
    let target_capped = send(
        &app,
        "GET",
        "/expiry-alias/links/target-capped",
        &[],
        Body::empty(),
    )
    .await;
    let direct = send(
        &app,
        "GET",
        "/expiry-alias/links/direct",
        &[],
        Body::empty(),
    )
    .await;
    let chain = send(&app, "GET", "/expiry-alias/links/chain", &[], Body::empty()).await;
    assert!(target.headers().contains_key(header::EXPIRES));
    assert_eq!(
        target_capped.headers()[header::EXPIRES],
        target.headers()[header::EXPIRES],
        "alias response must inherit its resolved target cap"
    );
    assert_ne!(
        direct.headers()[header::EXPIRES],
        target.headers()[header::EXPIRES],
        "direct alias policy must remain an independent earlier cap"
    );
    assert_eq!(
        chain.headers()[header::EXPIRES],
        direct.headers()[header::EXPIRES],
        "alias chains must inherit intermediate alias caps"
    );

    let item = send(
        &app,
        "GET",
        "/expiry-alias/directory/item.txt",
        &[],
        Body::empty(),
    )
    .await;
    let through_directory = send(
        &app,
        "GET",
        "/expiry-alias/view/item.txt",
        &[],
        Body::empty(),
    )
    .await;
    assert_eq!(
        through_directory.headers()[header::EXPIRES],
        item.headers()[header::EXPIRES],
        "directory alias descendants must inherit the resolved file cap"
    );

    let dangling = send(&app, "GET", "/expiry-alias/dangling", &[], Body::empty()).await;
    assert_eq!(dangling.status(), StatusCode::NOT_FOUND);
    let dangling_report = send(
        &app,
        "GET",
        "/expiry-alias/dangling/EXPIRES",
        &[],
        Body::empty(),
    )
    .await;
    assert_eq!(dangling_report.status(), StatusCode::OK);
    let (_, dangling_report) = json(dangling_report).await;
    assert!(dangling_report["effective_expires_at"].is_string());
}

#[tokio::test]
#[expect(clippy::too_many_lines)]
async fn phase_five_content_mutations_report_and_store_sanitized_bytes() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::new(root.path().to_path_buf()).unwrap();
    store
        .put_file("redacted", "replace.txt", b"before")
        .unwrap();
    store
        .put_file("redacted", "splice.txt", b"prefix:")
        .unwrap();
    let app = router(App::new(store.clone()));
    let management = format!("sym_mgmt_{}", "a".repeat(64));
    let claim = format!("sym_claim_{}", "b".repeat(64));
    let redacted_management = format!("sym_mgmt_{}", "*".repeat(64));
    let redacted_claim = format!("sym_claim_{}", "*".repeat(64));

    let allocated = send(
        &app,
        "POST",
        "/redacted/generated/",
        &[("file-extension", "txt")],
        format!("{management}\n{claim}"),
    )
    .await;
    assert_eq!(allocated.status(), StatusCode::CREATED);
    assert_eq!(allocated.headers()["sanitized-management-tokens"], "1");
    assert_eq!(allocated.headers()["sanitized-creator-claims"], "1");
    let (_, allocated_json) = json(allocated).await;
    assert_eq!(allocated_json["sanitized_management_tokens"], 1);
    assert_eq!(allocated_json["sanitized_creator_claims"], 1);
    let allocation_bytes = format!("{redacted_management}\n{redacted_claim}");
    let allocation_hash = blake3::hash(allocation_bytes.as_bytes())
        .to_hex()
        .to_string();
    assert_eq!(allocated_json["hash"], format!("blake3:{allocation_hash}"));
    assert!(
        allocated_json["path"]
            .as_str()
            .unwrap()
            .contains(&allocation_hash)
    );
    let fetched = fetch_absolute_location(&app, allocated_json["url"].as_str().unwrap()).await;
    assert_eq!(
        to_bytes(fetched.into_body(), usize::MAX).await.unwrap(),
        allocation_bytes
    );

    let store::Node::File {
        hash: replacement_base,
        ..
    } = store.lookup("redacted", "replace.txt").unwrap()
    else {
        panic!("replacement fixture must be a file");
    };
    let replaced = send(
        &app,
        "REPLACE",
        "/redacted/replace.txt",
        &[("if-content-match", &replacement_base.to_wire())],
        claim,
    )
    .await;
    assert_eq!(replaced.status(), StatusCode::OK);
    assert_eq!(replaced.headers()["sanitized-creator-claims"], "1");
    let (_, replaced_json) = json(replaced).await;
    assert_eq!(replaced_json["sanitized_creator_claims"], 1);
    assert_eq!(
        replaced_json["new_hash"],
        format!(
            "blake3:{}",
            blake3::hash(redacted_claim.as_bytes()).to_hex()
        )
    );

    let store::Node::File {
        hash: splice_base, ..
    } = store.lookup("redacted", "splice.txt").unwrap()
    else {
        panic!("splice fixture must be a file");
    };
    let spliced = send(
        &app,
        "PATCH",
        "/redacted/splice.txt",
        &[
            ("if-content-match", &splice_base.to_wire()),
            (
                "splice",
                &format!("offset=7; delete=0; insert={}", management.len()),
            ),
        ],
        management,
    )
    .await;
    assert_eq!(spliced.status(), StatusCode::OK);
    assert_eq!(spliced.headers()["sanitized-management-tokens"], "1");
    let (_, spliced_json) = json(spliced).await;
    assert_eq!(spliced_json["sanitized_management_tokens"], 1);
    let expected_splice = format!("prefix:{redacted_management}");
    assert_eq!(
        spliced_json["new_hash"],
        format!(
            "blake3:{}",
            blake3::hash(expected_splice.as_bytes()).to_hex()
        )
    );
    let fetched = fetch_absolute_location(&app, spliced_json["location"].as_str().unwrap()).await;
    assert_eq!(
        to_bytes(fetched.into_body(), usize::MAX).await.unwrap(),
        expected_splice
    );
}

#[tokio::test]
async fn splice_result_limit_and_stale_guard_precede_materialization() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::new(root.path().to_path_buf()).unwrap();
    store.put_file("bounded", "data.bin", b"abcd").unwrap();
    let store::Node::File { hash, .. } = store.lookup("bounded", "data.bin").unwrap() else {
        panic!("fixture path is a file");
    };
    let app = router(App::with_max_file_size(store.clone(), 4));
    let too_large = send(
        &app,
        "PATCH",
        "/bounded/data.bin",
        &[
            ("if-content-match", &hash.to_wire()),
            ("splice", "offset=4; delete=0; insert=1"),
        ],
        "x",
    )
    .await;
    assert_eq!(too_large.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(
        store.read_blob(hash).unwrap().as_ref(),
        b"abcd",
        "failed splice must not replace the source"
    );
    assert_eq!(
        std::fs::read_dir(root.path().join("tmp")).unwrap().count(),
        0
    );

    store.put_file("bounded", "data.bin", b"xy").unwrap();
    let stale = send(
        &app,
        "PATCH",
        "/bounded/data.bin",
        &[
            ("if-content-match", &hash.to_wire()),
            ("splice", "offset=999; delete=0; insert=0"),
        ],
        Body::empty(),
    )
    .await;
    assert_eq!(stale.status(), StatusCode::PRECONDITION_FAILED);
    assert!(stale.headers().contains_key("content-revision"));
    // The stale-hash ETag carries the blake3: prefix, like every other content
    // ETag, and matches the hash named in the body.
    let current = ContentHash::from(blake3::hash(b"xy"));
    assert_eq!(
        stale.headers()[header::ETAG],
        format!("\"{}\"", current.to_wire()).as_str()
    );
    let body = to_bytes(stale.into_body(), usize::MAX).await.unwrap();
    let body = String::from_utf8(body.to_vec()).unwrap();
    assert!(
        body.contains(&current.to_wire()),
        "stale body must name the current hash in wire form: {body}"
    );
}

#[tokio::test]
async fn managed_phase_five_mutations_reject_before_spooling() {
    let (root, _store, app) = fixture(Fixture::Managed).await;
    for (method, path, headers) in [
        ("POST", "/hello/generated/", Vec::new()),
        (
            "REPLACE",
            "/hello/index.html",
            vec![("if-content-match", "0".repeat(64))],
        ),
        (
            "PATCH",
            "/hello/index.html",
            vec![
                ("if-content-match", "0".repeat(64)),
                ("splice", "offset=0; delete=0; insert=1".to_string()),
            ],
        ),
    ] {
        let borrowed = headers
            .iter()
            .map(|(name, value)| (*name, value.as_str()))
            .collect::<Vec<_>>();
        let response = send(&app, method, path, &borrowed, "must not spool").await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            std::fs::read_dir(root.path().join("tmp")).unwrap().count(),
            0
        );
    }
}

#[tokio::test]
async fn pending_allocations_remain_bound_to_the_proposing_bearer() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::new(root.path().to_path_buf()).unwrap();
    store.put_file("managed", "index.html", b"site").unwrap();
    let original = store.operator_claim("managed").unwrap();
    let app = router(App::new(store.clone()));
    let original_authorization = format!("Bearer {}", original.encode());
    let proposed = send(
        &app,
        "POST",
        "/managed/custom/",
        &[
            ("allocation-action", "propose"),
            ("authorization", &original_authorization),
            ("idempotency-key", "bearer-bound-proposal"),
        ],
        "pending",
    )
    .await;
    assert_eq!(proposed.status(), StatusCode::ACCEPTED);
    let (_, proposed) = json(proposed).await;
    let token = proposed["allocation_token"].as_str().unwrap();

    let replacement = store.operator_rotate("managed", &original).unwrap();
    let replacement_authorization = format!("Bearer {}", replacement.encode());
    let finalized = send(
        &app,
        "POST",
        "/managed/custom/",
        &[
            ("allocation-action", "finalize"),
            ("allocation-token", token),
            ("file-name", "chosen"),
            ("authorization", &replacement_authorization),
            ("idempotency-key", "bearer-bound-finalize"),
        ],
        Body::empty(),
    )
    .await;
    assert_eq!(finalized.status(), StatusCode::UNAUTHORIZED);
    assert!(matches!(
        store.lookup("managed", "custom/chosen"),
        Err(StoreError::NotFound)
    ));
}

#[tokio::test]
#[expect(clippy::too_many_lines)]
async fn every_phase_five_endpoint_exercises_stale_noop_conflict_and_limits() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::new(root.path().to_path_buf()).unwrap();
    store.put_file("edge", "data.bin", b"data").unwrap();
    let stale_tree = store.site_inventory("edge").unwrap().tree_hash;
    let store::Node::File { hash, .. } = store.lookup("edge", "data.bin").unwrap() else {
        panic!("fixture path is a file");
    };
    store.put_file("edge", "later.bin", b"later").unwrap();
    let app = router(App::new(store.clone()));

    for (method, path, headers, body) in [
        (
            "ALIAS",
            "/edge/link",
            vec![
                ("alias-target", "data.bin".to_string()),
                ("if-match", stale_tree.clone()),
            ],
            String::new(),
        ),
        (
            "ALIAS",
            "/edge/",
            vec![
                ("content-type", "application/json".to_string()),
                ("if-match", stale_tree.clone()),
            ],
            r#"{"aliases":[{"path":"link","target":"data.bin"}]}"#.to_string(),
        ),
        (
            "POST",
            "/edge/generated/",
            vec![("if-match", stale_tree.clone())],
            "stale allocation".to_string(),
        ),
        (
            "REPLACE",
            "/edge/data.bin",
            vec![
                ("if-content-match", hash.to_wire()),
                ("if-match", stale_tree.clone()),
            ],
            "replace".to_string(),
        ),
        (
            "PATCH",
            "/edge/data.bin",
            vec![
                ("if-content-match", hash.to_wire()),
                ("if-match", stale_tree.clone()),
                ("splice", "offset=0; delete=0; insert=1".to_string()),
            ],
            "x".to_string(),
        ),
    ] {
        let borrowed = headers
            .iter()
            .map(|(name, value)| (*name, value.as_str()))
            .collect::<Vec<_>>();
        let response = send(&app, method, path, &borrowed, body).await;
        assert_eq!(
            response.status(),
            StatusCode::PRECONDITION_FAILED,
            "{method} stale tree guard"
        );
        assert!(response.headers().contains_key("content-revision"));
    }
    assert!(matches!(
        store.lookup("edge", "generated"),
        Err(StoreError::NotFound) | Ok(store::Node::Dir)
    ));
    assert_eq!(
        std::fs::read_dir(root.path().join("tmp")).unwrap().count(),
        0
    );

    let replace_noop = send(
        &app,
        "REPLACE",
        "/edge/data.bin",
        &[
            ("if-content-match", &hash.to_wire()),
            ("idempotency-key", "edge-replace-noop"),
        ],
        "data",
    )
    .await;
    assert_eq!(replace_noop.status(), StatusCode::OK);
    assert!(!replace_noop.headers().contains_key("undo-token"));
    let replace_noop_etag = replace_noop.headers()["etag"].clone();
    store.put_file("edge", "after-noop.bin", b"after").unwrap();
    let replace_noop_replay = send(
        &app,
        "REPLACE",
        "/edge/data.bin",
        &[
            ("if-content-match", &hash.to_wire()),
            ("idempotency-key", "edge-replace-noop"),
        ],
        "data",
    )
    .await;
    assert_eq!(replace_noop_replay.headers()["etag"], replace_noop_etag);
    assert_eq!(
        replace_noop_replay.headers()["idempotency-replayed"],
        "true"
    );
    let splice_noop = send(
        &app,
        "PATCH",
        "/edge/data.bin",
        &[
            ("if-content-match", &hash.to_wire()),
            ("splice", "offset=0; delete=0; insert=0"),
            ("idempotency-key", "edge-splice-noop"),
        ],
        Body::empty(),
    )
    .await;
    assert_eq!(splice_noop.status(), StatusCode::OK);
    assert!(!splice_noop.headers().contains_key("undo-token"));
    let splice_noop_etag = splice_noop.headers()["etag"].clone();
    store
        .put_file("edge", "after-splice-noop.bin", b"after")
        .unwrap();
    let splice_noop_replay = send(
        &app,
        "PATCH",
        "/edge/data.bin",
        &[
            ("if-content-match", &hash.to_wire()),
            ("splice", "offset=0; delete=0; insert=0"),
            ("idempotency-key", "edge-splice-noop"),
        ],
        Body::empty(),
    )
    .await;
    assert_eq!(splice_noop_replay.headers()["etag"], splice_noop_etag);
    assert_eq!(splice_noop_replay.headers()["idempotency-replayed"], "true");

    let batch_body =
        r#"{"aliases":[{"path":"one","target":"data.bin"},{"path":"two","target":"data.bin"}]}"#;
    let first_batch = send(
        &app,
        "ALIAS",
        "/edge/",
        &[
            ("content-type", "application/json"),
            ("idempotency-key", "edge-batch-replay"),
        ],
        batch_body,
    )
    .await;
    assert_eq!(first_batch.status(), StatusCode::CREATED);
    let replay_batch = send(
        &app,
        "ALIAS",
        "/edge/",
        &[
            ("content-type", "application/json"),
            ("idempotency-key", "edge-batch-replay"),
        ],
        batch_body,
    )
    .await;
    assert_eq!(replay_batch.status(), StatusCode::CREATED);
    assert_eq!(replay_batch.headers()["idempotency-replayed"], "true");
    let noop_batch = send(
        &app,
        "ALIAS",
        "/edge/",
        &[
            ("content-type", "application/json"),
            ("idempotency-key", "edge-batch-noop"),
        ],
        batch_body,
    )
    .await;
    assert_eq!(noop_batch.status(), StatusCode::OK);
    assert!(!noop_batch.headers().contains_key("undo-token"));

    let conflict = send(
        &app,
        "ALIAS",
        "/edge/",
        &[("content-type", "application/json")],
        r#"{"aliases":[{"path":"rollback","target":"data.bin"},{"path":"data.bin","target":"later.bin"}]}"#,
    )
    .await;
    assert_eq!(conflict.status(), StatusCode::CONFLICT);
    assert!(matches!(
        store.alias("edge", "rollback"),
        Err(StoreError::NotFound)
    ));

    let aliases = (0..=4096)
        .map(|index| contract::AliasDefinition {
            path: format!("limit/{index}"),
            target: "data.bin".to_string(),
        })
        .collect::<Vec<_>>();
    let oversized_batch = serde_json::to_string(&contract::AliasBatchRequest { aliases }).unwrap();
    let batch_limit = send(
        &app,
        "ALIAS",
        "/edge/",
        &[("content-type", "application/json")],
        oversized_batch,
    )
    .await;
    assert_eq!(batch_limit.status(), StatusCode::PAYLOAD_TOO_LARGE);
    let oversized_target = "x".repeat(upload::MAX_ALIAS_TARGET_BYTES + 1);
    let target_limit = send(
        &app,
        "ALIAS",
        "/edge/too-long",
        &[("alias-target", &oversized_target)],
        Body::empty(),
    )
    .await;
    assert_eq!(target_limit.status(), StatusCode::PAYLOAD_TOO_LARGE);

    let limited = router(App::with_max_file_size(store, 4));
    let allocation_limit = send(&limited, "POST", "/edge/generated/", &[], "12345").await;
    assert_eq!(allocation_limit.status(), StatusCode::PAYLOAD_TOO_LARGE);
    let replace_limit = send(
        &limited,
        "REPLACE",
        "/edge/data.bin",
        &[("if-content-match", &hash.to_wire())],
        "12345",
    )
    .await;
    assert_eq!(replace_limit.status(), StatusCode::PAYLOAD_TOO_LARGE);
}
