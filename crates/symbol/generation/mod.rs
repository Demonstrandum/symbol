pub mod docs;
mod ledger;
mod provenance;
mod swc_generation;

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

pub use ledger::{ApiVersion, SourceHash, VersionLedger, preview_ledger, read_ledger};
pub use provenance::{BuildProvenance, COMMIT_ENV, DIRTY_ENV, git_watch_paths, tracked_git_files};
use serde::{Deserialize, Serialize};
use serde_json::json;
use thiserror::Error;

use self::ledger::{lock_workspace, reconcile_ledger, write_atomic};
use self::swc_generation::{emit_declarations, emit_javascript_module, emit_typescript, emit_umd};

pub const GENERATOR_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const INITIAL_API_VERSION: ApiVersion = ApiVersion::new(0, 1, 0);
pub const UPDATE_COMMAND: &str = "./api-version update";
pub const GENERATION_MODE_ENV: &str = "SYMBOL_GENERATION_MODE";
pub const BUMP_INTENT_ENV: &str = "SYMBOL_API_BUMP";
pub const BUMP_EXPECTED_ENV: &str = "SYMBOL_API_BUMP_EXPECTED";
pub const BUMP_TARGET_ENV: &str = "SYMBOL_API_BUMP_TARGET";

const HASH_DOMAIN: &[u8] = b"symbol-api-canonical-inputs-v3\0";
const CANONICAL_FIXED_INPUTS: &[&str] = &[
    "API.md",
    "api-version",
    "static/api.ts",
    "static/api.py",
    "crates/symbol/build.rs",
    "crates/symbol/Cargo.toml",
    "Cargo.lock",
];
const CANONICAL_SOURCE_DIRECTORIES: &[&str] = &["crates/symbol/generation"];
const CANONICAL_WATCH_DIRECTORIES: &[&str] = &[
    "crates/symbol-contract/src",
    "crates/symbol/generation",
    "static",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GenerationMode {
    Update,
    ReadOnly,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BumpIntent {
    Automatic,
    Explicit(ExplicitBumpRequest),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BumpKind {
    Minor,
    Major,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExplicitBumpRequest {
    pub kind: BumpKind,
    pub expected: VersionLedger,
    pub target: ApiVersion,
}

#[derive(Debug, Error)]
pub enum GenerationError {
    #[error("I/O error for {path}: {source}")]
    FileIo {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("invalid semantic API version `{0}`; expected X.Y.Z")]
    InvalidVersion(String),
    #[error("invalid Blake3 source hash `{0}`")]
    InvalidSourceHash(String),
    #[error("absolute API revision must be greater than zero")]
    InvalidAbsoluteRevision,
    #[error("invalid api-version.toml at {path}: {source}")]
    LedgerToml {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },
    #[error(
        "api-version.toml is stale (recorded {recorded}, canonical {actual}); update from a writable checkout with `{update_command}`"
    )]
    StaleLedger {
        recorded: SourceHash,
        actual: SourceHash,
        update_command: &'static str,
    },
    #[error(
        "{path} is stale in read-only generation mode; update from a writable checkout with `{update_command}`"
    )]
    StaleSnapshot {
        path: PathBuf,
        update_command: &'static str,
    },
    #[error("invalid output path {0}")]
    InvalidOutputPath(PathBuf),
    #[error("invalid {GENERATION_MODE_ENV} value `{0}`; expected `update` or `readonly`")]
    InvalidGenerationMode(String),
    #[error("invalid {BUMP_INTENT_ENV} value `{0}`; expected `minor` or `major`")]
    InvalidBumpIntent(String),
    #[error(
        "{BUMP_INTENT_ENV}, {BUMP_EXPECTED_ENV}, and {BUMP_TARGET_ENV} must be provided together"
    )]
    IncompleteBumpEnvironment,
    #[error("invalid {BUMP_EXPECTED_ENV}; expected VERSION:REVISION:SOURCE_HASH")]
    InvalidBumpExpected,
    #[error("{BUMP_TARGET_ENV} does not match the requested semantic bump")]
    InvalidBumpTarget,
    #[error("{BUMP_INTENT_ENV} requires writable generation mode")]
    BumpRequiresUpdateMode,
    #[error(
        "explicit API bump conflicted: expected {expected}, but observed {observed}; rerun the wrapper against the current ledger"
    )]
    BumpConflict { expected: String, observed: String },
    #[error("SWC generation failed: {0}")]
    Swc(String),
    #[error("unresolved template placeholder in {0}")]
    UnresolvedTemplatePlaceholder(PathBuf),
    #[error("git command could not start: {source}")]
    GitIo {
        #[source]
        source: std::io::Error,
    },
    #[error("{command} failed: {stderr}")]
    GitCommand {
        command: &'static str,
        stderr: String,
    },
    #[error("{COMMIT_ENV} and {DIRTY_ENV} must be provided together")]
    IncompleteProvenanceEnvironment,
    #[error("{0} is not valid Unicode")]
    InvalidProvenanceEnvironment(&'static str),
    #[error("{DIRTY_ENV} must be `true` or `false`, got `{0}`")]
    InvalidDirtyValue(String),
    #[error("build commit must be 7-64 raw hexadecimal characters or `unknown`")]
    InvalidGitCommit,
    #[error("invalid Cargo.lock at {path}: {source}")]
    CargoLockToml {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },
    #[error("invalid generator manifest at {path}: {source}")]
    GeneratorManifestToml {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },
    #[error("generator dependency `{0}` is missing or ambiguous in Cargo.lock")]
    GeneratorDependencyResolution(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GenerationMetadata {
    pub ledger: VersionLedger,
    pub provenance: BuildProvenance,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedArtifacts {
    pub api_ts: String,
    pub api_js: String,
    pub api_global_js: String,
    pub api_d_ts: String,
    pub api_py: String,
    pub contract_fixture_json: String,
}

#[derive(Debug, Serialize)]
struct VersionedContractFixture {
    fixture_version: u32,
    extension_normalization: &'static [symbol_contract::ExtensionNormalizationVector],
    api_version: String,
    absolute_revision: u64,
    source_hash: String,
    generator_version: &'static str,
    build: BuildProvenanceFixture,
    response_identity_headers: [&'static str; 3],
    splice: SpliceContractFixture,
    operations: Vec<symbol_contract::OperationFixture>,
}

#[derive(Debug, Serialize)]
struct BuildProvenanceFixture {
    commit: String,
    dirty: bool,
}

#[derive(Debug, Serialize)]
struct SpliceContractFixture {
    media_type: &'static str,
    header_max_bytes: usize,
    header_max_descriptors: usize,
    frame_max_descriptors: usize,
    frame_max_metadata_bytes: usize,
}

impl GeneratedArtifacts {
    /// Writes every generated artifact without touching unchanged files.
    ///
    /// # Errors
    ///
    /// Returns an error if the output directory or an artifact cannot be written.
    pub fn write_to(&self, out_dir: &Path) -> Result<(), GenerationError> {
        fs::create_dir_all(out_dir).map_err(|source| GenerationError::FileIo {
            path: out_dir.to_path_buf(),
            source,
        })?;
        write_if_changed(&out_dir.join("symbol.ts"), self.api_ts.as_bytes())?;
        write_if_changed(&out_dir.join("symbol.js"), self.api_js.as_bytes())?;
        write_if_changed(
            &out_dir.join("symbol.global.js"),
            self.api_global_js.as_bytes(),
        )?;
        write_if_changed(&out_dir.join("symbol.d.ts"), self.api_d_ts.as_bytes())?;
        write_if_changed(&out_dir.join("symbol.py"), self.api_py.as_bytes())?;
        write_if_changed(
            &out_dir.join("symbol-contract.json"),
            self.contract_fixture_json.as_bytes(),
        )
    }
}

/// Selects writable update or read-only verification mode once per build.
///
/// # Errors
///
/// Returns an error when `SYMBOL_GENERATION_MODE` has an unsupported value.
pub fn generation_mode_from_environment() -> Result<GenerationMode, GenerationError> {
    match env::var(GENERATION_MODE_ENV) {
        Ok(value) => match value.as_str() {
            "update" => Ok(GenerationMode::Update),
            "readonly" => Ok(GenerationMode::ReadOnly),
            _ => Err(GenerationError::InvalidGenerationMode(value)),
        },
        Err(env::VarError::NotPresent) if env::var_os("NIX_BUILD_TOP").is_some() => {
            Ok(GenerationMode::ReadOnly)
        }
        Err(env::VarError::NotPresent) => Ok(GenerationMode::Update),
        Err(env::VarError::NotUnicode(value)) => Err(GenerationError::InvalidGenerationMode(
            value.to_string_lossy().into_owned(),
        )),
    }
}

/// Reads the explicit semantic bump selected by the wrapper before Cargo starts.
///
/// # Errors
///
/// Returns an error when `SYMBOL_API_BUMP` is neither `minor` nor `major`.
pub fn bump_intent_from_environment() -> Result<BumpIntent, GenerationError> {
    let kind = optional_unicode_environment(BUMP_INTENT_ENV)?;
    let expected = optional_unicode_environment(BUMP_EXPECTED_ENV)?;
    let target = optional_unicode_environment(BUMP_TARGET_ENV)?;
    match (kind, expected, target) {
        (None, None, None) => Ok(BumpIntent::Automatic),
        (Some(kind), Some(expected), Some(target)) => {
            let kind = match kind.as_str() {
                "minor" => BumpKind::Minor,
                "major" => BumpKind::Major,
                _ => return Err(GenerationError::InvalidBumpIntent(kind)),
            };
            let expected = parse_bump_expected(&expected)?;
            let target = target
                .parse::<ApiVersion>()
                .map_err(|_| GenerationError::InvalidBumpTarget)?;
            let calculated_target = match kind {
                BumpKind::Minor => expected.version.next_minor(),
                BumpKind::Major => expected.version.next_major(),
            };
            if target != calculated_target {
                return Err(GenerationError::InvalidBumpTarget);
            }
            Ok(BumpIntent::Explicit(ExplicitBumpRequest {
                kind,
                expected,
                target,
            }))
        }
        _ => Err(GenerationError::IncompleteBumpEnvironment),
    }
}

fn optional_unicode_environment(name: &'static str) -> Result<Option<String>, GenerationError> {
    match env::var(name) {
        Ok(value) => Ok(Some(value)),
        Err(env::VarError::NotPresent) => Ok(None),
        Err(env::VarError::NotUnicode(_)) => {
            Err(GenerationError::InvalidProvenanceEnvironment(name))
        }
    }
}

fn parse_bump_expected(value: &str) -> Result<VersionLedger, GenerationError> {
    let mut fields = value.split(':');
    let version = fields
        .next()
        .ok_or(GenerationError::InvalidBumpExpected)?
        .parse::<ApiVersion>()
        .map_err(|_| GenerationError::InvalidBumpExpected)?;
    let absolute_revision = fields
        .next()
        .ok_or(GenerationError::InvalidBumpExpected)?
        .parse::<u64>()
        .map_err(|_| GenerationError::InvalidBumpExpected)?;
    let source_hash = SourceHash::new(
        fields
            .next()
            .ok_or(GenerationError::InvalidBumpExpected)?
            .to_string(),
    )
    .map_err(|_| GenerationError::InvalidBumpExpected)?;
    if fields.next().is_some() || absolute_revision == 0 {
        return Err(GenerationError::InvalidBumpExpected);
    }
    Ok(VersionLedger {
        version,
        absolute_revision,
        source_hash,
    })
}

/// Creates, updates, or verifies the API version ledger for one build.
///
/// # Errors
///
/// Returns an error if canonical inputs or the ledger cannot be read, or if a
/// read-only build observes stale state.
pub fn prepare_ledger(root: &Path, mode: GenerationMode) -> Result<VersionLedger, GenerationError> {
    prepare_ledger_with_intent(root, mode, BumpIntent::Automatic)
}

/// Reconciles the ledger with a bump intent selected before generation starts.
///
/// A dirty-source explicit bump combines the source and semantic change in one
/// absolute revision. An explicit bump of an already-recorded source hash is a
/// separate semantic operation and therefore consumes another revision.
///
/// # Errors
///
/// Returns an error if canonical inputs or the ledger cannot be read or updated,
/// or if an explicit bump is requested in read-only mode.
pub fn prepare_ledger_with_intent(
    root: &Path,
    mode: GenerationMode,
    intent: BumpIntent,
) -> Result<VersionLedger, GenerationError> {
    reconcile_ledger(root, mode, intent)
}

/// Returns every file included in the canonical API source hash.
///
/// # Errors
///
/// Returns an error if a required canonical input is missing or unreadable.
pub fn canonical_input_paths(root: &Path) -> Result<Vec<PathBuf>, GenerationError> {
    let mut paths = CANONICAL_FIXED_INPUTS
        .iter()
        .map(|relative| root.join(relative))
        .collect::<Vec<_>>();
    for relative in CANONICAL_SOURCE_DIRECTORIES {
        collect_rust_sources(&root.join(relative), &mut paths)?;
    }
    paths.sort_by_key(|path| logical_path(root, path));
    for path in &paths {
        if !path.is_file() {
            return Err(GenerationError::FileIo {
                path: path.clone(),
                source: std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "canonical input is missing",
                ),
            });
        }
    }
    Ok(paths)
}

/// Returns canonical directories watched as directories so newly added inputs rerun Cargo.
#[must_use]
pub fn canonical_input_directories(root: &Path) -> Vec<PathBuf> {
    CANONICAL_WATCH_DIRECTORIES
        .iter()
        .map(|relative| root.join(relative))
        .collect()
}

/// Computes the framed Blake3 hash of all canonical API inputs.
///
/// # Errors
///
/// Returns an error if a canonical input cannot be discovered or read.
pub fn canonical_source_hash(root: &Path) -> Result<SourceHash, GenerationError> {
    let mut hasher = blake3::Hasher::new();
    hasher.update(HASH_DOMAIN);
    for path in canonical_input_paths(root)? {
        let relative = logical_path(root, &path);
        if relative == "Cargo.lock" || relative == "crates/symbol/Cargo.toml" {
            continue;
        }
        let bytes = fs::read(&path).map_err(|source| GenerationError::FileIo {
            path: path.clone(),
            source,
        })?;
        hash_framed_input(&mut hasher, &relative, &bytes);
    }
    hash_framed_input(
        &mut hasher,
        "symbol-contract.fixture.json",
        symbol_contract::contract_fixture_json().as_bytes(),
    );
    hash_framed_input(
        &mut hasher,
        "Cargo.lock:generator-resolution-v1",
        &generator_dependency_fingerprint(root)?,
    );
    SourceHash::new(hasher.finalize().to_hex().to_string())
}

/// Updates a checked source snapshot locally or rejects stale read-only state.
///
/// # Errors
///
/// Returns an error if the snapshot cannot be read or atomically replaced, or
/// if read-only mode observes stale content.
pub fn reconcile_snapshot(
    root: &Path,
    path: &Path,
    expected: &str,
    mode: GenerationMode,
) -> Result<(), GenerationError> {
    let current = read_optional_string(path)?;
    if current.as_deref() == Some(expected) {
        return Ok(());
    }
    match mode {
        GenerationMode::ReadOnly => Err(GenerationError::StaleSnapshot {
            path: path.to_path_buf(),
            update_command: UPDATE_COMMAND,
        }),
        GenerationMode::Update => {
            let _lock = lock_workspace(root)?;
            let current = read_optional_string(path)?;
            if current.as_deref() != Some(expected) {
                write_atomic(path, expected.as_bytes())?;
            }
            Ok(())
        }
    }
}

/// Writes a generated output file only when its contents changed.
///
/// # Errors
///
/// Returns an error if the output cannot be atomically written.
pub fn write_generated_file(path: &Path, contents: &str) -> Result<(), GenerationError> {
    write_if_changed(path, contents.as_bytes())
}

/// Generates the TypeScript, JavaScript, declaration, Python, and contract artifacts.
///
/// # Errors
///
/// Returns an error if templates cannot be read, placeholders remain unresolved,
/// or SWC cannot parse or transform the TypeScript template.
pub fn generate_artifacts(
    root: &Path,
    metadata: &GenerationMetadata,
) -> Result<GeneratedArtifacts, GenerationError> {
    metadata.provenance.validate()?;
    let ts_path = root.join("static/api.ts");
    let py_path = root.join("static/api.py");
    let ts_template = fs::read_to_string(&ts_path).map_err(|source| GenerationError::FileIo {
        path: ts_path.clone(),
        source,
    })?;
    let py_template = fs::read_to_string(&py_path).map_err(|source| GenerationError::FileIo {
        path: py_path.clone(),
        source,
    })?;

    // The template is not parsed on its own. Every placeholder sits inside a
    // string literal and is replaced by another string literal, so the rendered
    // source below is the same program structurally, and parsing it catches the
    // same syntax errors without a second full SWC pass per build.
    let typescript_source =
        render_typescript_template(&ts_template, "symbol.ts", metadata, &ts_path)?;
    let module_source = render_typescript_template(&ts_template, "symbol.js", metadata, &ts_path)?;
    let global_source =
        render_typescript_template(&ts_template, "symbol.global.js", metadata, &ts_path)?;
    let declarations_source =
        render_typescript_template(&ts_template, "symbol.d.ts", metadata, &ts_path)?;
    let python_source = render_python_template(&py_template, "symbol.py", metadata, &py_path)?;

    let typescript_artifact = with_header(
        "//",
        "symbol.ts",
        metadata,
        &emit_typescript(&typescript_source, "symbol.ts")?,
    );
    let module_artifact = with_header(
        "//",
        "symbol.js",
        metadata,
        &emit_javascript_module(&module_source, "symbol.js")?,
    );
    let global_artifact = with_header(
        "//",
        "symbol.global.js",
        metadata,
        &emit_umd(&global_source)?,
    );
    let declarations_artifact = with_header(
        "//",
        "symbol.d.ts",
        metadata,
        &emit_declarations(&declarations_source, "symbol.d.ts")?,
    );
    let python_artifact = with_header("#", "symbol.py", metadata, &python_source);

    Ok(GeneratedArtifacts {
        api_ts: typescript_artifact,
        api_js: module_artifact,
        api_global_js: global_artifact,
        api_d_ts: declarations_artifact,
        api_py: python_artifact,
        contract_fixture_json: versioned_contract_fixture_json(metadata),
    })
}

fn versioned_contract_fixture(metadata: &GenerationMetadata) -> VersionedContractFixture {
    let contract = symbol_contract::contract_fixture();
    VersionedContractFixture {
        fixture_version: contract.fixture_version,
        extension_normalization: contract.extension_normalization,
        api_version: metadata.ledger.version.to_string(),
        absolute_revision: metadata.ledger.absolute_revision,
        source_hash: metadata.ledger.source_hash.as_str().to_string(),
        generator_version: GENERATOR_VERSION,
        build: BuildProvenanceFixture {
            commit: metadata.provenance.commit.clone(),
            dirty: metadata.provenance.dirty,
        },
        response_identity_headers: [
            "Symbol-API-Version",
            "Symbol-API-Revision",
            "Symbol-API-Source-Hash",
        ],
        splice: SpliceContractFixture {
            media_type: symbol_contract::SPLICE_MEDIA_TYPE,
            header_max_bytes: symbol_contract::SPLICE_HEADER_MAX_BYTES,
            header_max_descriptors: symbol_contract::SPLICE_HEADER_MAX_DESCRIPTORS,
            frame_max_descriptors: symbol_contract::SPLICE_FRAME_MAX_DESCRIPTORS,
            frame_max_metadata_bytes: symbol_contract::SPLICE_FRAME_MAX_METADATA_BYTES,
        },
        operations: contract.operations,
    }
}

fn versioned_contract_fixture_json(metadata: &GenerationMetadata) -> String {
    let mut fixture = serde_json::to_string_pretty(&versioned_contract_fixture(metadata))
        .expect("versioned contract fixture is serializable");
    fixture.push('\n');
    fixture
}

#[derive(Debug, Deserialize)]
struct GeneratorManifest {
    package: GeneratorPackage,
    #[serde(rename = "build-dependencies")]
    build_dependencies: BTreeMap<String, DependencySpecification>,
}

#[derive(Debug, Deserialize)]
struct GeneratorPackage {
    version: String,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum DependencySpecification {
    Version(String),
    Detailed(DetailedDependency),
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DetailedDependency {
    version: Option<String>,
    package: Option<String>,
    path: Option<String>,
    git: Option<String>,
    branch: Option<String>,
    tag: Option<String>,
    rev: Option<String>,
    registry: Option<String>,
    #[serde(default = "default_true", rename = "default-features")]
    default_features: bool,
    #[serde(default)]
    features: Vec<String>,
    #[serde(default)]
    optional: bool,
}

#[derive(Debug, Deserialize)]
struct CargoLock {
    package: Vec<LockedPackage>,
}

#[derive(Debug, Deserialize)]
struct LockedPackage {
    name: String,
    version: String,
    source: Option<String>,
    checksum: Option<String>,
    #[serde(default)]
    dependencies: Vec<String>,
}

fn generator_dependency_fingerprint(root: &Path) -> Result<Vec<u8>, GenerationError> {
    let manifest_path = root.join("crates/symbol/Cargo.toml");
    let manifest_source =
        fs::read_to_string(&manifest_path).map_err(|source| GenerationError::FileIo {
            path: manifest_path.clone(),
            source,
        })?;
    let manifest: GeneratorManifest = toml::from_str(&manifest_source).map_err(|source| {
        GenerationError::GeneratorManifestToml {
            path: manifest_path,
            source,
        }
    })?;
    let mut fingerprint = format!("generator-package\0{}\n", manifest.package.version).into_bytes();
    let mut roots = BTreeSet::new();
    for (alias, specification) in &manifest.build_dependencies {
        let dependency = NormalizedDependency::new(alias, specification);
        roots.insert(dependency.package.to_string());
        fingerprint.extend_from_slice(dependency.record().as_bytes());
    }

    let lock_path = root.join("Cargo.lock");
    let lock_source = fs::read_to_string(&lock_path).map_err(|source| GenerationError::FileIo {
        path: lock_path.clone(),
        source,
    })?;
    let lock: CargoLock =
        toml::from_str(&lock_source).map_err(|source| GenerationError::CargoLockToml {
            path: lock_path,
            source,
        })?;

    let mut selected = BTreeSet::new();
    let mut pending = VecDeque::new();
    for root_name in roots {
        pending.push_back(resolve_locked_package(&lock.package, &root_name)?);
    }
    while let Some(index) = pending.pop_front() {
        if !selected.insert(index) {
            continue;
        }
        for dependency in &lock.package[index].dependencies {
            pending.push_back(resolve_locked_package(&lock.package, dependency)?);
        }
    }

    let mut packages = selected
        .into_iter()
        .map(|index| &lock.package[index])
        .collect::<Vec<_>>();
    packages.sort_by(|left, right| locked_package_key(left).cmp(&locked_package_key(right)));
    for package in packages {
        let mut dependencies = package.dependencies.clone();
        dependencies.sort();
        let record = format!(
            "{}\0{}\0{}\0{}\0{}\n",
            package.name,
            package.version,
            package.source.as_deref().unwrap_or("path"),
            package.checksum.as_deref().unwrap_or(""),
            dependencies.join("\0"),
        );
        fingerprint.extend_from_slice(record.as_bytes());
    }
    Ok(fingerprint)
}

struct NormalizedDependency<'a> {
    alias: &'a str,
    package: &'a str,
    version: &'a str,
    path: &'a str,
    git: &'a str,
    branch: &'a str,
    tag: &'a str,
    rev: &'a str,
    registry: &'a str,
    default_features: bool,
    features: Vec<&'a str>,
    optional: bool,
}

impl<'a> NormalizedDependency<'a> {
    fn new(alias: &'a str, specification: &'a DependencySpecification) -> Self {
        match specification {
            DependencySpecification::Version(version) => Self {
                alias,
                package: alias,
                version,
                path: "",
                git: "",
                branch: "",
                tag: "",
                rev: "",
                registry: "",
                default_features: true,
                features: Vec::new(),
                optional: false,
            },
            DependencySpecification::Detailed(dependency) => {
                let mut features = dependency
                    .features
                    .iter()
                    .map(String::as_str)
                    .collect::<Vec<_>>();
                features.sort_unstable();
                Self {
                    alias,
                    package: dependency.package.as_deref().unwrap_or(alias),
                    version: dependency.version.as_deref().unwrap_or(""),
                    path: dependency.path.as_deref().unwrap_or(""),
                    git: dependency.git.as_deref().unwrap_or(""),
                    branch: dependency.branch.as_deref().unwrap_or(""),
                    tag: dependency.tag.as_deref().unwrap_or(""),
                    rev: dependency.rev.as_deref().unwrap_or(""),
                    registry: dependency.registry.as_deref().unwrap_or(""),
                    default_features: dependency.default_features,
                    features,
                    optional: dependency.optional,
                }
            }
        }
    }

    fn record(&self) -> String {
        format!(
            "build-dependency\0{}\0{}\0{}\0{}\0{}\0{}\0{}\0{}\0{}\0{}\0{}\0{}\n",
            self.alias,
            self.package,
            self.version,
            self.path,
            self.git,
            self.branch,
            self.tag,
            self.rev,
            self.registry,
            self.default_features,
            self.optional,
            self.features.join("\0"),
        )
    }
}

const fn default_true() -> bool {
    true
}

fn resolve_locked_package(
    packages: &[LockedPackage],
    dependency: &str,
) -> Result<usize, GenerationError> {
    let without_source = dependency
        .split_once(" (")
        .map_or(dependency, |(prefix, _)| prefix);
    let mut components = without_source.split_whitespace();
    let name = components.next().unwrap_or(dependency);
    let version = components.next();
    let matches = packages
        .iter()
        .enumerate()
        .filter(|(_, package)| {
            package.name == name && version.is_none_or(|version| package.version == version)
        })
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    if matches.len() != 1 {
        return Err(GenerationError::GeneratorDependencyResolution(
            dependency.to_string(),
        ));
    }
    Ok(matches[0])
}

fn locked_package_key(package: &LockedPackage) -> (&str, &str, &str) {
    (
        &package.name,
        &package.version,
        package.source.as_deref().unwrap_or("path"),
    )
}

fn collect_rust_sources(directory: &Path, paths: &mut Vec<PathBuf>) -> Result<(), GenerationError> {
    let mut entries = fs::read_dir(directory)
        .map_err(|source| GenerationError::FileIo {
            path: directory.to_path_buf(),
            source,
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|source| GenerationError::FileIo {
            path: directory.to_path_buf(),
            source,
        })?;
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        let path = entry.path();
        let file_type = entry
            .file_type()
            .map_err(|source| GenerationError::FileIo {
                path: path.clone(),
                source,
            })?;
        if file_type.is_dir() {
            collect_rust_sources(&path, paths)?;
        } else if is_canonical_rust_source(&path) {
            paths.push(path);
        }
    }
    Ok(())
}

fn is_canonical_rust_source(path: &Path) -> bool {
    path.extension().is_some_and(|extension| extension == "rs")
        && path.file_name().is_some_and(|name| {
            let name = name.to_string_lossy();
            name != "tests.rs" && !name.ends_with("_tests.rs")
        })
}

fn logical_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .expect("canonical inputs stay beneath the workspace root")
        .to_string_lossy()
        .replace('\\', "/")
}

fn hash_framed_input(hasher: &mut blake3::Hasher, name: &str, contents: &[u8]) {
    hasher.update(
        &u64::try_from(name.len())
            .expect("input name length fits u64")
            .to_be_bytes(),
    );
    hasher.update(name.as_bytes());
    hasher.update(
        &u64::try_from(contents.len())
            .expect("input length fits u64")
            .to_be_bytes(),
    );
    hasher.update(contents);
}

fn render_typescript_template(
    template: &str,
    artifact: &str,
    metadata: &GenerationMetadata,
    template_path: &Path,
) -> Result<String, GenerationError> {
    let mut rendered = template.to_string();
    let metadata_json = json!({
        "artifact": artifact,
        "apiVersion": metadata.ledger.version.to_string(),
        "absoluteRevision": metadata.ledger.absolute_revision,
        "sourceHash": metadata.ledger.source_hash.as_str(),
        "generatorVersion": GENERATOR_VERSION,
        "commit": metadata.provenance.commit.as_str(),
        "dirty": metadata.provenance.dirty,
    })
    .to_string();
    replace(&mut rendered, "\"{METADATA_JSON}\"", &json!(metadata_json));
    let contract_fixture_json = json!(versioned_contract_fixture(metadata)).to_string();
    replace(
        &mut rendered,
        "\"{CONTRACT_FIXTURE_JSON}\"",
        &json!(contract_fixture_json),
    );
    ensure_no_placeholders(&rendered, template_path)?;
    Ok(rendered)
}

fn render_python_template(
    template: &str,
    artifact: &str,
    metadata: &GenerationMetadata,
    template_path: &Path,
) -> Result<String, GenerationError> {
    let mut rendered = template.to_string();
    let metadata_json = json!({
        "artifact": artifact,
        "api_version": metadata.ledger.version.to_string(),
        "absolute_revision": metadata.ledger.absolute_revision,
        "source_hash": metadata.ledger.source_hash.as_str(),
        "generator_version": GENERATOR_VERSION,
        "commit": metadata.provenance.commit.as_str(),
        "dirty": metadata.provenance.dirty,
    })
    .to_string();
    rendered = rendered.replace("{METADATA}", &metadata_json);
    ensure_no_placeholders(&rendered, template_path)?;
    Ok(rendered)
}

fn replace(rendered: &mut String, placeholder: &str, value: &serde_json::Value) {
    *rendered = rendered.replace(placeholder, &value.to_string());
}

fn ensure_no_placeholders(rendered: &str, path: &Path) -> Result<(), GenerationError> {
    if rendered.contains("__SYMBOL_")
        || rendered.contains("{METADATA}")
        || rendered.contains("{METADATA_JSON}")
        || rendered.contains("{CONTRACT_FIXTURE_JSON}")
    {
        return Err(GenerationError::UnresolvedTemplatePlaceholder(
            path.to_path_buf(),
        ));
    }
    Ok(())
}

fn with_header(
    comment: &str,
    artifact: &str,
    metadata: &GenerationMetadata,
    source: &str,
) -> String {
    let state = if metadata.provenance.dirty {
        "dirty"
    } else {
        "clean"
    };
    format!(
        "{comment} @generated by symbol-generation; DO NOT EDIT.\n\
         {comment} Artifact: {artifact}\n\
         {comment} API: {}\n\
         {comment} Absolute revision: {}\n\
         {comment} Source Blake3: {}\n\
         {comment} Generator version: {GENERATOR_VERSION}\n\
         {comment} Commit: {}\n\
         {comment} State: {state}\n\n\
         {source}",
        metadata.ledger.version,
        metadata.ledger.absolute_revision,
        metadata.ledger.source_hash,
        metadata.provenance.commit,
    )
}

fn write_if_changed(path: &Path, contents: &[u8]) -> Result<(), GenerationError> {
    if read_optional_bytes(path)?.as_deref() == Some(contents) {
        return Ok(());
    }
    write_atomic(path, contents)
}

fn read_optional_string(path: &Path) -> Result<Option<String>, GenerationError> {
    match fs::read_to_string(path) {
        Ok(contents) => Ok(Some(contents)),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(GenerationError::FileIo {
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn read_optional_bytes(path: &Path) -> Result<Option<Vec<u8>>, GenerationError> {
    match fs::read(path) {
        Ok(contents) => Ok(Some(contents)),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(GenerationError::FileIo {
            path: path.to_path_buf(),
            source,
        }),
    }
}

#[cfg(test)]
mod ledger_process_tests;
#[cfg(test)]
mod tests;
