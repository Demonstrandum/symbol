#[derive(Debug, serde::Serialize)]
pub struct EndpointContract {
    pub name: &'static str,
    pub method: &'static str,
    pub head: bool,
    pub path: &'static str,
    pub success_statuses: &'static [u16],
    pub error_statuses: &'static [u16],
    pub request_headers: &'static [&'static str],
    pub response_headers: &'static [&'static str],
}

macro_rules! endpoint {
    ($name:literal, $method:literal, $head:literal, $path:literal,
     $success:expr, $errors:expr, $request:expr, $response:expr) => {
        EndpointContract {
            name: $name,
            method: $method,
            head: $head,
            path: $path,
            success_statuses: $success,
            error_statuses: $errors,
            request_headers: $request,
            response_headers: $response,
        }
    };
}

pub const ROOT: &str = "/";
pub const HASH: &str = "/HASH";
pub const STATS: &str = "/STATS";
pub const STATS_SLASH: &str = "/STATS/";
pub const FILES: &str = "/FILES";
pub const FILES_SLASH: &str = "/FILES/";
pub const INSTALL: &str = "/install.sh";
pub const INSTALL_HASH: &str = "/install.sh/HASH";
pub const CLIENT: &str = "/symbol.sh";
pub const CLIENT_HASH: &str = "/symbol.sh/HASH";
pub const SITE_FILES: &str = "/{name}/FILES";
pub const SITE_FILES_SLASH: &str = "/{name}/FILES/";
pub const SITE_FILES_PATH: &str = "/{name}/FILES/{*path}";
pub const SITE_UNDO: &str = "/{name}/UNDO";
pub const SITE_UNDO_SLASH: &str = "/{name}/UNDO/";
pub const SITE_EXPIRES: &str = "/{name}/EXPIRES";
pub const SITE_EXPIRES_SLASH: &str = "/{name}/EXPIRES/";
pub const SITE_ROOT: &str = "/{name}/";
pub const IMMUTABLE_BLOB: &str = "/.blob/{name}/{hash}";
pub const SITE_PATH: &str = "/{name}/{*path}";
pub const SITE: &str = "/{name}";

pub const METHOD_COPY: &str = "COPY";
pub const METHOD_MOVE: &str = "MOVE";
pub const METHOD_UNDO: &str = "UNDO";
pub const METHOD_EXPIRE: &str = "EXPIRE";
pub const METHOD_MANAGE: &str = "MANAGE";

const READ_ERRORS: &[u16] = &[304, 400, 404, 416];
const MUTATION_ERRORS: &[u16] = &[400, 401, 403, 404, 409, 412, 413, 500];
const MUTATION_HEADERS: &[&str] = &[
    "Location",
    "ETag",
    "Content-Revision",
    "Undo-Token",
    "Undo-Expires",
    "Creator-Claim",
    "Management-Token",
    "Sanitized-Management-Tokens",
    "Sanitized-Creator-Claims",
];

pub static ENDPOINTS: &[EndpointContract] = &[
    endpoint!(
        "docs",
        "GET",
        true,
        "/",
        &[200, 304],
        &[304],
        &["Accept", "If-None-Match"],
        &["Content-Type", "ETag", "Cache-Control"]
    ),
    endpoint!(
        "unnamed put",
        "PUT",
        false,
        "/",
        &[201],
        MUTATION_ERRORS,
        &[
            "Content-Type",
            "Content-Disposition",
            "Unpack",
            "Idempotency-Key",
            "Creator-Claim",
            "Management-Action"
        ],
        MUTATION_HEADERS
    ),
    endpoint!(
        "docs hash",
        "GET",
        true,
        "/HASH",
        &[200],
        &[404],
        &[],
        &["Content-Type"]
    ),
    endpoint!(
        "stats",
        "GET",
        true,
        "/STATS[/]",
        &[200],
        &[500],
        &[],
        &["Content-Type"]
    ),
    endpoint!(
        "installer",
        "GET",
        true,
        "/install.sh",
        &[200, 304],
        &[304],
        &["If-None-Match"],
        &["Content-Type", "ETag", "Cache-Control"]
    ),
    endpoint!(
        "installer hash",
        "GET",
        true,
        "/install.sh/HASH",
        &[200],
        &[404],
        &[],
        &["Content-Type"]
    ),
    endpoint!(
        "client",
        "GET",
        true,
        "/symbol.sh",
        &[200, 304],
        &[304],
        &["If-None-Match"],
        &["Content-Type", "ETag", "Cache-Control"]
    ),
    endpoint!(
        "client hash",
        "GET",
        true,
        "/symbol.sh/HASH",
        &[200],
        &[404],
        &[],
        &["Content-Type"]
    ),
    endpoint!(
        "site listing",
        "GET",
        true,
        "/FILES[/]",
        &[200, 304],
        &[304, 500],
        &["Accept", "If-None-Match"],
        &["Content-Type", "ETag", "Cache-Control"]
    ),
    endpoint!(
        "site redirect",
        "GET",
        true,
        "/{name}",
        &[307],
        &[400, 404],
        &[],
        &["Location"]
    ),
    endpoint!(
        "site index",
        "GET",
        true,
        "/{name}/",
        &[200, 304, 307],
        READ_ERRORS,
        &["If-None-Match", "Range", "If-Range"],
        &[
            "Content-Type",
            "Content-Length",
            "ETag",
            "Cache-Control",
            "Expires"
        ]
    ),
    endpoint!(
        "site put",
        "PUT",
        false,
        "/{name}[/]",
        &[200, 201],
        MUTATION_ERRORS,
        &[
            "Authorization",
            "Content-Type",
            "Content-Disposition",
            "Unpack",
            "If-Match",
            "Idempotency-Key",
            "Creator-Claim",
            "Management-Action"
        ],
        MUTATION_HEADERS
    ),
    endpoint!(
        "site pop",
        "DELETE",
        false,
        "/{name}[/]",
        &[200],
        &[400, 401, 404, 500],
        &["Authorization"],
        &[
            "Content-Type",
            "Content-Disposition",
            "Undo-Token",
            "Undo-Expires"
        ]
    ),
    endpoint!(
        "site copy",
        "COPY",
        false,
        "/{name}[/]",
        &[201],
        &[400, 404, 409, 500],
        &[
            "Destination",
            "Idempotency-Key",
            "Creator-Claim",
            "Management-Action"
        ],
        MUTATION_HEADERS
    ),
    endpoint!(
        "site move",
        "MOVE",
        false,
        "/{name}[/]",
        &[200],
        &[400, 401, 404, 409, 500],
        &["Authorization", "Destination"],
        MUTATION_HEADERS
    ),
    endpoint!(
        "site undo",
        "UNDO",
        false,
        "/{name}[/]",
        &[200],
        &[401, 404, 409, 500],
        &["Authorization", "Undo-Token"],
        &["Content-Type"]
    ),
    endpoint!(
        "site expire",
        "EXPIRE",
        false,
        "/{name}[/]",
        &[200],
        &[400, 401, 404, 500],
        &[
            "Authorization",
            "Expiry-Mode",
            "Expiry-In",
            "Expiry-At",
            "Expiry-Min-Age",
            "Expiry-Max-Age",
            "Expiry-Max-Size",
            "Expiry-Power"
        ],
        &[
            "Content-Type",
            "Expires",
            "Expiry-Mode",
            "Undo-Token",
            "Undo-Expires"
        ]
    ),
    endpoint!(
        "site management",
        "MANAGE",
        false,
        "/{name}[/]",
        &[200],
        &[400, 401, 403, 404, 409, 500],
        &[
            "Authorization",
            "Creator-Claim",
            "Management-Action",
            "Idempotency-Key"
        ],
        &[
            "Content-Type",
            "Management-Token",
            "Idempotency-Replayed",
            "Cache-Control"
        ]
    ),
    endpoint!(
        "site file",
        "GET",
        true,
        "/{name}/{path...}",
        &[200, 206, 304, 307],
        READ_ERRORS,
        &["Range", "If-Range", "If-None-Match"],
        &[
            "Content-Type",
            "Content-Length",
            "Content-Range",
            "Accept-Ranges",
            "ETag",
            "Cache-Control",
            "Expires"
        ]
    ),
    endpoint!(
        "file put",
        "PUT",
        false,
        "/{name}/{path...}",
        &[200, 201],
        MUTATION_ERRORS,
        &["Authorization", "If-Match", "Content-Type"],
        MUTATION_HEADERS
    ),
    endpoint!(
        "file delete",
        "DELETE",
        false,
        "/{name}/{path...}",
        &[200],
        &[400, 401, 404, 500],
        &["Authorization"],
        &["Content-Type", "Undo-Token", "Undo-Expires"]
    ),
    endpoint!(
        "file expire",
        "EXPIRE",
        false,
        "/{name}/{path...}",
        &[200],
        &[400, 401, 404, 500],
        &[
            "Authorization",
            "Expiry-Mode",
            "Expiry-In",
            "Expiry-At",
            "Expiry-Min-Age",
            "Expiry-Max-Age",
            "Expiry-Max-Size",
            "Expiry-Power"
        ],
        &[
            "Content-Type",
            "Expires",
            "Expiry-Mode",
            "Undo-Token",
            "Undo-Expires"
        ]
    ),
    endpoint!(
        "archive get",
        "GET",
        true,
        "/{name}.{tar|tar.gz|zip}",
        &[200],
        &[400, 404, 500],
        &[],
        &[
            "Content-Type",
            "Content-Length",
            "Content-Disposition",
            "Cache-Control"
        ]
    ),
    endpoint!(
        "archive pop",
        "DELETE",
        false,
        "/{name}.{tar|tar.gz|zip}",
        &[200],
        &[400, 401, 404, 500],
        &["Authorization"],
        &[
            "Content-Type",
            "Content-Length",
            "Content-Disposition",
            "Undo-Token",
            "Undo-Expires"
        ]
    ),
    endpoint!(
        "files inventory",
        "GET",
        true,
        "/{name}/FILES[/]",
        &[200, 304],
        &[304, 404, 500],
        &["Accept", "If-None-Match"],
        &["Content-Type", "ETag", "Content-Revision", "Cache-Control"]
    ),
    endpoint!(
        "files subtree",
        "GET",
        true,
        "/{name}/FILES/{path...}",
        &[200, 304, 307],
        &[304, 404, 500],
        &["Accept", "If-None-Match"],
        &["Content-Type", "ETag", "Cache-Control", "Location"]
    ),
    endpoint!(
        "file hash",
        "GET",
        true,
        "/{name}/{path...}/HASH",
        &[200],
        &[400, 404],
        &[],
        &["Content-Type"]
    ),
    endpoint!(
        "undo stack",
        "GET",
        true,
        "/{name}/UNDO[/]",
        &[200],
        &[404, 500],
        &[],
        &["Content-Type", "Cache-Control"]
    ),
    endpoint!(
        "expiry inventory",
        "GET",
        true,
        "/{name}/EXPIRES[/]",
        &[200],
        &[404, 500],
        &[],
        &["Content-Type", "Cache-Control"]
    ),
    endpoint!(
        "expiry target",
        "GET",
        true,
        "/{name}/{path...}/EXPIRES",
        &[200],
        &[400, 404, 500],
        &[],
        &["Content-Type", "Cache-Control"]
    ),
    endpoint!(
        "immutable blob",
        "GET",
        true,
        "/.blob/{name}/{hash}",
        &[200, 206, 304],
        READ_ERRORS,
        &["Range", "If-Range", "If-None-Match"],
        &[
            "Content-Type",
            "Content-Length",
            "Content-Range",
            "Accept-Ranges",
            "ETag",
            "Cache-Control"
        ]
    ),
];
