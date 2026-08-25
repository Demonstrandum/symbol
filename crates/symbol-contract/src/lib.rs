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

pub const METHOD_ALIAS: &str = "ALIAS";
pub const METHOD_COPY: &str = "COPY";
pub const METHOD_REPLACE: &str = "REPLACE";
pub const METHOD_MOVE: &str = "MOVE";
pub const METHOD_UNDO: &str = "UNDO";
pub const METHOD_EXPIRE: &str = "EXPIRE";
pub const METHOD_MANAGE: &str = "MANAGE";
pub const RESERVED_MUTATION_ERROR: &str = "error: path is reserved by symbol";
pub const SPLICE_MEDIA_TYPE: &str = "application/vnd.symbol.splice; version=1";

const READ_ERRORS: &[u16] = &[304, 400, 404, 416];
const MUTATION_ERRORS: &[u16] = &[400, 401, 403, 404, 409, 412, 413, 500];
const MUTATION_HEADERS: &[&str] = &[
    "Content-Type",
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
            "Content-Length",
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
    endpoint!(
        "alias batch",
        "ALIAS",
        false,
        "/{name}/",
        &[200, 201],
        &[400, 401, 403, 404, 409, 412, 413, 500],
        &[
            "Alias-Target",
            "Authorization",
            "Content-Type",
            "Idempotency-Key",
            "If-Match"
        ],
        &[
            "Content-Revision",
            "Content-Type",
            "ETag",
            "Idempotency-Replayed",
            "Location",
            "Undo-Expires",
            "Undo-Token"
        ]
    ),
    endpoint!(
        "alias file",
        "ALIAS",
        false,
        "/{name}/{path...}",
        &[200, 201],
        &[400, 401, 403, 404, 409, 412, 413, 500],
        &[
            "Alias-Target",
            "Authorization",
            "Idempotency-Key",
            "If-Match"
        ],
        &[
            "Content-Revision",
            "Content-Type",
            "ETag",
            "Idempotency-Replayed",
            "Location",
            "Undo-Expires",
            "Undo-Token"
        ]
    ),
    endpoint!(
        "allocated file",
        "POST",
        false,
        "/{name}/{folder...}/",
        &[200, 201, 202],
        &[400, 401, 403, 404, 409, 412, 413, 500],
        &[
            "Allocation-Action",
            "Allocation-Token",
            "Authorization",
            "Content-Type",
            "Expiry-At",
            "Expiry-In",
            "Expiry-Max-Age",
            "Expiry-Max-Size",
            "Expiry-Min-Age",
            "Expiry-Mode",
            "Expiry-Power",
            "File-Extension",
            "File-Name",
            "File-Prefix",
            "File-Suffix",
            "Idempotency-Key",
            "If-Match"
        ],
        &[
            "Content-Location",
            "Content-Revision",
            "Content-Type",
            "ETag",
            "Idempotency-Replayed",
            "Location",
            "Undo-Expires",
            "Undo-Token"
        ]
    ),
    endpoint!(
        "file replace",
        "REPLACE",
        false,
        "/{name}/{path...}",
        &[200],
        &[400, 401, 403, 404, 409, 412, 413, 500],
        &[
            "Authorization",
            "Content-Type",
            "Idempotency-Key",
            "If-Content-Match",
            "If-Match"
        ],
        &[
            "Content-Revision",
            "Content-Type",
            "ETag",
            "Idempotency-Replayed",
            "Location",
            "Undo-Expires",
            "Undo-Token"
        ]
    ),
    endpoint!(
        "file splice",
        "PATCH",
        false,
        "/{name}/{path...}",
        &[200],
        &[400, 401, 403, 404, 409, 412, 413, 416, 500],
        &[
            "Authorization",
            "Content-Type",
            "Idempotency-Key",
            "If-Content-Match",
            "If-Match",
            "Splice"
        ],
        &[
            "Content-Revision",
            "Content-Type",
            "ETag",
            "Idempotency-Replayed",
            "Location",
            "Undo-Expires",
            "Undo-Token"
        ]
    ),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WireBody {
    Empty,
    Json,
    PlainText,
    Binary,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct OutcomeContract {
    pub status: u16,
    pub body: WireBody,
    pub required_headers: &'static [&'static str],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct EndpointOutcomes {
    pub name: &'static str,
    pub success: &'static [OutcomeContract],
    pub errors: &'static [OutcomeContract],
}

const JSON_MUTATION_HEADERS: &[&str] = &["Content-Type", "Location", "ETag", "Content-Revision"];
const PLAIN_ERROR: WireBody = WireBody::PlainText;
const MUTATION_ERROR_OUTCOMES: &[OutcomeContract] = &[
    error_outcome(400),
    error_outcome(401),
    error_outcome(403),
    error_outcome(404),
    error_outcome(409),
    error_outcome(412),
    error_outcome(413),
    error_outcome(500),
];
const SPLICE_ERROR_OUTCOMES: &[OutcomeContract] = &[
    error_outcome(400),
    error_outcome(401),
    error_outcome(403),
    error_outcome(404),
    error_outcome(409),
    error_outcome(412),
    error_outcome(413),
    error_outcome(416),
    error_outcome(500),
];

pub static EXACT_OUTCOMES: &[EndpointOutcomes] = &[
    EndpointOutcomes {
        name: "alias batch",
        success: &[
            OutcomeContract {
                status: 200,
                body: WireBody::Json,
                required_headers: JSON_MUTATION_HEADERS,
            },
            OutcomeContract {
                status: 201,
                body: WireBody::Json,
                required_headers: JSON_MUTATION_HEADERS,
            },
        ],
        errors: MUTATION_ERROR_OUTCOMES,
    },
    EndpointOutcomes {
        name: "alias file",
        success: &[
            OutcomeContract {
                status: 200,
                body: WireBody::Json,
                required_headers: JSON_MUTATION_HEADERS,
            },
            OutcomeContract {
                status: 201,
                body: WireBody::Json,
                required_headers: JSON_MUTATION_HEADERS,
            },
        ],
        errors: MUTATION_ERROR_OUTCOMES,
    },
    EndpointOutcomes {
        name: "allocated file",
        success: &[
            OutcomeContract {
                status: 200,
                body: WireBody::Json,
                required_headers: JSON_MUTATION_HEADERS,
            },
            OutcomeContract {
                status: 201,
                body: WireBody::Json,
                required_headers: JSON_MUTATION_HEADERS,
            },
            OutcomeContract {
                status: 202,
                body: WireBody::Json,
                required_headers: &["Content-Type", "Location", "ETag", "Content-Revision"],
            },
        ],
        errors: MUTATION_ERROR_OUTCOMES,
    },
    EndpointOutcomes {
        name: "file replace",
        success: &[OutcomeContract {
            status: 200,
            body: WireBody::Json,
            required_headers: JSON_MUTATION_HEADERS,
        }],
        errors: MUTATION_ERROR_OUTCOMES,
    },
    EndpointOutcomes {
        name: "file splice",
        success: &[OutcomeContract {
            status: 200,
            body: WireBody::Json,
            required_headers: JSON_MUTATION_HEADERS,
        }],
        errors: SPLICE_ERROR_OUTCOMES,
    },
];

const fn error_outcome(status: u16) -> OutcomeContract {
    OutcomeContract {
        status,
        body: PLAIN_ERROR,
        required_headers: &["Content-Type"],
    }
}

#[derive(Debug, Clone, Copy, serde::Serialize)]
pub struct ServingStats {
    pub cache: CacheStats,
    pub readers: ReaderStats,
}

#[derive(Debug, Clone, Copy, serde::Serialize)]
pub struct CacheStats {
    pub hits: u64,
    pub misses: u64,
    pub evictions: u64,
}

#[derive(Debug, Clone, Copy, serde::Serialize)]
pub struct ReaderStats {
    pub operations: u64,
    pub waits: u64,
    pub wait_micros: u64,
    pub query_micros: u64,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct SizeDistribution {
    pub min: Option<u64>,
    pub p25: Option<f64>,
    pub median: Option<f64>,
    pub mean: Option<f64>,
    pub p75: Option<f64>,
    pub max: Option<u64>,
    pub iqr: Option<f64>,
    pub stddev: Option<f64>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Stats {
    pub sites: u64,
    pub files: u64,
    pub aliases: u64,
    pub blobs: u64,
    pub bytes: u64,
    pub logical_bytes: u64,
    pub saved_bytes: u64,
    pub saved_fraction: f64,
    pub file_sizes: SizeDistribution,
    pub blob_sizes: SizeDistribution,
    pub serving: ServingStats,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct ManagementStatus {
    pub managed: bool,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct InventoryFile {
    pub path: String,
    pub hash: String,
    pub size: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AliasTargetKind {
    File,
    Directory,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub struct InventoryAlias {
    pub path: String,
    pub target: String,
    pub target_kind: Option<AliasTargetKind>,
    pub dangling: bool,
    pub resolved_hash: Option<String>,
    pub size: Option<u64>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct SiteInventory {
    pub site: String,
    pub content_revision: u64,
    pub tree_hash: String,
    pub files: Vec<InventoryFile>,
    pub aliases: Vec<InventoryAlias>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct UndoEntry {
    pub token: String,
    pub kind: String,
    pub description: String,
    pub created_at: String,
    pub expires_at: String,
    pub remaining_seconds: u64,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct UndoStack {
    pub site: String,
    pub entries: Vec<UndoEntry>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ExpiryMode {
    Relative,
    Absolute,
    Decay,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ExpiryTargetKind {
    Site,
    Folder,
    File,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub struct ExpiryTarget {
    pub site: String,
    pub path: Option<String>,
    pub kind: ExpiryTargetKind,
}

#[derive(Debug, Clone, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct OwnExpiryReport {
    pub mode: ExpiryMode,
    pub min_age_seconds: Option<u64>,
    pub max_age_seconds: Option<u64>,
    pub max_size_bytes: Option<u64>,
    pub power: Option<f64>,
    pub retention_seconds: Option<u64>,
    pub expires_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub struct InheritedExpiryCap {
    pub kind: ExpiryTargetKind,
    pub path: Option<String>,
    pub expires_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub struct ExpiryLimit {
    pub kind: ExpiryTargetKind,
    pub path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct ExpiryReport {
    pub target: ExpiryTarget,
    pub size: u64,
    pub refreshed_at: Option<String>,
    pub own_policy: Option<OwnExpiryReport>,
    pub inherited_caps: Vec<InheritedExpiryCap>,
    pub effective_expires_at: Option<String>,
    pub remaining_seconds: Option<u64>,
    pub limited_by: Option<ExpiryLimit>,
}

#[derive(Debug, Clone, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct ExpirySiteReport {
    pub site: String,
    pub entries: Vec<ExpiryReport>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ListingKind {
    Site,
    Directory,
    File,
    Alias,
}

#[derive(Debug, Clone)]
pub struct ListingEntry {
    pub kind: ListingKind,
    pub name: String,
    pub files: Option<u64>,
    pub bytes: u64,
    pub target: Option<String>,
    pub target_kind: Option<AliasTargetKind>,
    pub dangling: Option<bool>,
}

impl serde::Serialize for ListingEntry {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct as _;

        match self.kind {
            ListingKind::Site | ListingKind::Directory => {
                let mut entry = serializer.serialize_struct("ListingEntry", 4)?;
                entry.serialize_field("kind", &self.kind)?;
                entry.serialize_field("name", &self.name)?;
                entry.serialize_field("files", &self.files)?;
                entry.serialize_field("bytes", &self.bytes)?;
                entry.end()
            }
            ListingKind::File => {
                let mut entry = serializer.serialize_struct("ListingEntry", 3)?;
                entry.serialize_field("kind", &self.kind)?;
                entry.serialize_field("name", &self.name)?;
                entry.serialize_field("bytes", &self.bytes)?;
                entry.end()
            }
            ListingKind::Alias => {
                let dangling = self.dangling.unwrap_or(true);
                let bytes = (!dangling).then_some(self.bytes);
                let mut entry = serializer.serialize_struct("ListingEntry", 7)?;
                entry.serialize_field("kind", &self.kind)?;
                entry.serialize_field("name", &self.name)?;
                entry.serialize_field("target", &self.target)?;
                entry.serialize_field("target_kind", &self.target_kind)?;
                entry.serialize_field("dangling", &dangling)?;
                entry.serialize_field("files", &self.files)?;
                entry.serialize_field("bytes", &bytes)?;
                entry.end()
            }
        }
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Listing {
    pub path: String,
    pub files: u64,
    pub aliases: u64,
    pub bytes: u64,
    pub entries: Vec<ListingEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct AliasDefinition {
    pub path: String,
    pub target: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct AliasBatchRequest {
    pub aliases: Vec<AliasDefinition>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct UndoReceipt {
    pub token: String,
    pub expires_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct MutationReceipt {
    pub changed: bool,
    pub replayed: bool,
    pub idempotency_key: String,
    pub location: String,
    pub etag: String,
    pub content_revision: u64,
    pub sanitized_management_tokens: u64,
    pub sanitized_creator_claims: u64,
    pub undo: Option<UndoReceipt>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct AliasReceipt {
    pub path: String,
    pub target: String,
    pub target_kind: Option<AliasTargetKind>,
    pub dangling: bool,
    pub resolved_hash: Option<String>,
    pub size: Option<u64>,
    #[serde(flatten)]
    pub mutation: MutationReceipt,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct AliasBatchReceipt {
    pub aliases: Vec<InventoryAlias>,
    #[serde(flatten)]
    pub mutation: MutationReceipt,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AllocationOutcome {
    Created,
    Existing,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "mode", rename_all = "lowercase")]
pub enum AllocationNaming {
    Generated {
        prefix: String,
        extension: String,
        suffix: String,
    },
    Custom,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct AllocatedFileReceipt {
    pub outcome: AllocationOutcome,
    pub site: String,
    pub path: String,
    pub name: String,
    pub url: String,
    pub hash: String,
    pub size: u64,
    pub blob_url: String,
    pub naming: AllocationNaming,
    #[serde(flatten)]
    pub mutation: MutationReceipt,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ProposedFileName {
    pub folder: String,
    pub default_name: String,
    pub hash: String,
    pub size: u64,
    pub media_type: String,
    pub inferred_extension: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct AllocationProposalReceipt {
    pub allocation_token: String,
    pub expires_at: String,
    pub proposal: ProposedFileName,
    pub idempotency_key: String,
    pub replayed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct AllocationCancellationReceipt {
    pub allocation_token: String,
    pub cancelled: bool,
    pub idempotency_key: String,
    pub replayed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ReplacementOutcome {
    Replaced,
    Relocated,
    Unchanged,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct FileReplaceReceipt {
    pub outcome: ReplacementOutcome,
    pub old_path: String,
    pub new_path: String,
    pub relocated: bool,
    pub old_hash: String,
    pub new_hash: String,
    pub size: u64,
    #[serde(flatten)]
    pub mutation: MutationReceipt,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct SpliceReceipt {
    pub old_path: String,
    pub new_path: String,
    pub relocated: bool,
    pub old_hash: String,
    pub new_hash: String,
    pub old_size: u64,
    pub new_size: u64,
    pub splices: usize,
    #[serde(flatten)]
    pub mutation: MutationReceipt,
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeSet, HashSet};

    use super::{ENDPOINTS, EXACT_OUTCOMES};

    #[test]
    fn endpoint_names_are_unique_and_shapes_are_well_formed() {
        let mut names = HashSet::new();
        for endpoint in ENDPOINTS {
            assert!(names.insert(endpoint.name), "duplicate {}", endpoint.name);
            assert!(!endpoint.method.is_empty());
            assert!(endpoint.path.starts_with('/'));
            assert!(!endpoint.success_statuses.is_empty());
            assert!(
                endpoint
                    .success_statuses
                    .iter()
                    .chain(endpoint.error_statuses)
                    .all(|status| (100..=599).contains(status))
            );
            assert!(
                endpoint
                    .request_headers
                    .iter()
                    .chain(endpoint.response_headers)
                    .all(|header| !header.is_empty())
            );
        }
    }

    #[test]
    fn every_installed_frozen_addition_has_exact_outcomes() {
        let expected = [
            "alias batch",
            "alias file",
            "allocated file",
            "file replace",
            "file splice",
        ];
        assert_eq!(
            EXACT_OUTCOMES
                .iter()
                .map(|endpoint| endpoint.name)
                .collect::<BTreeSet<_>>(),
            expected.into_iter().collect()
        );
        for outcomes in EXACT_OUTCOMES {
            let endpoint = ENDPOINTS
                .iter()
                .find(|endpoint| endpoint.name == outcomes.name)
                .expect("exact outcomes name a frozen endpoint");
            let success = outcomes
                .success
                .iter()
                .map(|outcome| outcome.status)
                .collect::<BTreeSet<_>>();
            let errors = outcomes
                .errors
                .iter()
                .map(|outcome| outcome.status)
                .collect::<BTreeSet<_>>();
            assert_eq!(
                success,
                endpoint.success_statuses.iter().copied().collect(),
                "{} success outcomes",
                endpoint.name
            );
            assert_eq!(
                errors,
                endpoint.error_statuses.iter().copied().collect(),
                "{} error outcomes",
                endpoint.name
            );
        }
    }
}
