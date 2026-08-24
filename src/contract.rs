#[derive(Debug, serde::Serialize)]
pub struct EndpointContract {
    pub name: &'static str,
    pub methods: &'static [&'static str],
    pub path: &'static str,
    pub success_statuses: &'static [u16],
    pub request_headers: &'static [&'static str],
    pub response_headers: &'static [&'static str],
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

pub static ENDPOINTS: &[EndpointContract] = &[
    EndpointContract {
        name: "root",
        methods: &["GET", "HEAD", "PUT"],
        path: "/",
        success_statuses: &[200, 201],
        request_headers: &["Accept", "Content-Type", "Content-Disposition", "Unpack"],
        response_headers: &["Content-Type", "Location", "ETag", "Content-Revision"],
    },
    EndpointContract {
        name: "generated hashes",
        methods: &["GET", "HEAD"],
        path: "/HASH",
        success_statuses: &[200],
        request_headers: &[],
        response_headers: &["Content-Type"],
    },
    EndpointContract {
        name: "stats",
        methods: &["GET", "HEAD"],
        path: "/STATS",
        success_statuses: &[200],
        request_headers: &[],
        response_headers: &["Content-Type"],
    },
    EndpointContract {
        name: "client assets",
        methods: &["GET", "HEAD"],
        path: "/install.sh | /symbol.sh | */HASH",
        success_statuses: &[200, 304],
        request_headers: &["If-None-Match"],
        response_headers: &["Content-Type", "ETag", "Cache-Control"],
    },
    EndpointContract {
        name: "site listing",
        methods: &["GET", "HEAD"],
        path: "/FILES",
        success_statuses: &[200, 304],
        request_headers: &["Accept", "If-None-Match"],
        response_headers: &["Content-Type", "ETag", "Cache-Control"],
    },
    EndpointContract {
        name: "site root",
        methods: &[
            "GET", "HEAD", "PUT", "DELETE", "COPY", "MOVE", "UNDO", "EXPIRE", "MANAGE",
        ],
        path: "/{name}",
        success_statuses: &[200, 201, 307],
        request_headers: &[
            "Authorization",
            "Destination",
            "If-Match",
            "Idempotency-Key",
            "Management-Action",
        ],
        response_headers: &[
            "Location",
            "ETag",
            "Content-Revision",
            "Undo-Token",
            "Undo-Expires",
        ],
    },
    EndpointContract {
        name: "site directory",
        methods: &["GET", "HEAD"],
        path: "/{name}/",
        success_statuses: &[200, 307],
        request_headers: &["If-None-Match"],
        response_headers: &["Content-Type", "ETag", "Cache-Control", "Expires"],
    },
    EndpointContract {
        name: "site file",
        methods: &["GET", "HEAD", "PUT", "DELETE", "EXPIRE"],
        path: "/{name}/{path...}",
        success_statuses: &[200, 201, 206],
        request_headers: &["Authorization", "Range", "If-Range", "If-Match"],
        response_headers: &[
            "Content-Type",
            "Content-Length",
            "Content-Range",
            "Accept-Ranges",
            "ETag",
            "Expires",
        ],
    },
    EndpointContract {
        name: "archive get",
        methods: &["GET", "HEAD"],
        path: "/{name}.tar | .tar.gz | .zip",
        success_statuses: &[200],
        request_headers: &[],
        response_headers: &["Content-Type", "Content-Length", "Content-Disposition"],
    },
    EndpointContract {
        name: "archive pop",
        methods: &["DELETE"],
        path: "/{name}.tar | .tar.gz | .zip",
        success_statuses: &[200],
        request_headers: &["Authorization"],
        response_headers: &["Content-Type", "Content-Disposition", "Undo-Token"],
    },
    EndpointContract {
        name: "files inventory",
        methods: &["GET", "HEAD"],
        path: "/{name}/FILES",
        success_statuses: &[200, 304],
        request_headers: &["Accept", "If-None-Match"],
        response_headers: &["Content-Type", "ETag", "Content-Revision"],
    },
    EndpointContract {
        name: "files subtree",
        methods: &["GET", "HEAD"],
        path: "/{name}/FILES/{path...}",
        success_statuses: &[200, 304, 307],
        request_headers: &["Accept", "If-None-Match"],
        response_headers: &["Content-Type", "ETag"],
    },
    EndpointContract {
        name: "file hash",
        methods: &["GET", "HEAD"],
        path: "/{name}/{path...}/HASH",
        success_statuses: &[200],
        request_headers: &[],
        response_headers: &["Content-Type"],
    },
    EndpointContract {
        name: "undo stack",
        methods: &["GET", "HEAD"],
        path: "/{name}/UNDO",
        success_statuses: &[200],
        request_headers: &[],
        response_headers: &["Content-Type", "Cache-Control"],
    },
    EndpointContract {
        name: "undo restore",
        methods: &["UNDO"],
        path: "/{name}",
        success_statuses: &[200],
        request_headers: &["Authorization", "Undo-Token"],
        response_headers: &["Content-Type"],
    },
    EndpointContract {
        name: "expiry inventory",
        methods: &["GET", "HEAD"],
        path: "/{name}/EXPIRES",
        success_statuses: &[200],
        request_headers: &[],
        response_headers: &["Content-Type", "Cache-Control"],
    },
    EndpointContract {
        name: "expiry report",
        methods: &["GET", "HEAD"],
        path: "/{name}/{path...}/EXPIRES",
        success_statuses: &[200],
        request_headers: &[],
        response_headers: &["Content-Type", "Cache-Control"],
    },
    EndpointContract {
        name: "expiry mutation",
        methods: &["EXPIRE"],
        path: "/{name}[/{path...}]",
        success_statuses: &[200],
        request_headers: &["Authorization", "Expiry-Mode", "Expiry-In", "Expiry-At"],
        response_headers: &["Content-Type", "Expires", "Expiry-Mode", "Undo-Token"],
    },
    EndpointContract {
        name: "management",
        methods: &["MANAGE"],
        path: "/{name}",
        success_statuses: &[200],
        request_headers: &[
            "Authorization",
            "Creator-Claim",
            "Management-Action",
            "Idempotency-Key",
        ],
        response_headers: &[
            "Content-Type",
            "Management-Token",
            "Idempotency-Replayed",
            "Cache-Control",
        ],
    },
    EndpointContract {
        name: "immutable blob",
        methods: &["GET", "HEAD"],
        path: "/.blob/{name}/{hash}",
        success_statuses: &[200, 206, 304],
        request_headers: &["Range", "If-Range", "If-None-Match"],
        response_headers: &[
            "Content-Type",
            "Content-Length",
            "Content-Range",
            "Accept-Ranges",
            "ETag",
            "Cache-Control",
        ],
    },
];
