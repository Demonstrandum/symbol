export type ApiVersion = readonly [major: number, minor: number, patch: number];
export type Blake3 = string & { readonly __brand: "Blake3" };
export type GitCommit = string & { readonly __brand: "GitCommit" };
export type TreeHash = string;
export type EntityTag = string;
export type IdempotencyKey = string;
export type UndoToken = string;
export type ManagementToken = string;
export type CreatorClaim = string;
export type ApiArtifact = "symbol.ts" | "symbol.js" | "symbol.global.js" | "symbol.d.ts";

export interface SymbolApiMetadata {
  readonly artifact: ApiArtifact;
  readonly apiVersion: ApiVersion;
  readonly absoluteRevision: number;
  readonly sourceHash: Blake3;
  readonly generatorVersion: ApiVersion;
  readonly commit: GitCommit;
  readonly dirty: boolean;
}

export interface SymbolApiIdentity {
  readonly apiVersion: ApiVersion;
  readonly absoluteRevision: number;
  readonly sourceHash: Blake3;
}

export const METADATA_JSON: string = "{METADATA_JSON}";

function parseMetadata(source: string): Readonly<SymbolApiMetadata> {
  const value: unknown = JSON.parse(source);
  if (
    !isRecord(value) ||
    typeof value.artifact !== "string" ||
    typeof value.apiVersion !== "string" ||
    typeof value.absoluteRevision !== "number" ||
    typeof value.sourceHash !== "string" ||
    typeof value.generatorVersion !== "string" ||
    typeof value.commit !== "string" ||
    typeof value.dirty !== "boolean"
  ) {
    throw new TypeError("generated API metadata does not match its schema");
  }
  return Object.freeze({
    artifact: apiArtifact(value.artifact),
    apiVersion: apiVersion(value.apiVersion),
    absoluteRevision: value.absoluteRevision,
    sourceHash: blake3(value.sourceHash),
    generatorVersion: apiVersion(value.generatorVersion),
    commit: gitCommit(value.commit),
    dirty: value.dirty,
  });
}

export const metadata: Readonly<SymbolApiMetadata> = parseMetadata(METADATA_JSON);

export const API_VERSION_PARTS: ApiVersion = metadata.apiVersion;
export const API_VERSION: string = formatApiVersion(API_VERSION_PARTS);
export const API_REVISION: number = metadata.absoluteRevision;
export const SOURCE_HASH: Blake3 = metadata.sourceHash;
export const GENERATOR_VERSION_PARTS: ApiVersion = metadata.generatorVersion;
export const GENERATOR_VERSION: string = formatApiVersion(GENERATOR_VERSION_PARTS);
export const BUILD_COMMIT: GitCommit = metadata.commit;
export const BUILD_DIRTY: boolean = metadata.dirty;

export type EndpointName =
  | "docs"
  | "unnamed put"
  | "docs hash"
  | "stats"
  | "installer"
  | "installer hash"
  | "client"
  | "client hash"
  | "api client asset"
  | "api client hash"
  | "api documentation"
  | "api version"
  | "site listing"
  | "site redirect"
  | "site index"
  | "site put"
  | "site pop"
  | "site copy"
  | "site move"
  | "site undo"
  | "site expire"
  | "site management"
  | "site file"
  | "file put"
  | "file delete"
  | "file expire"
  | "archive get"
  | "archive pop"
  | "files inventory"
  | "files subtree"
  | "file hash"
  | "undo stack"
  | "expiry inventory"
  | "expiry target"
  | "immutable blob"
  | "alias batch"
  | "alias file"
  | "allocated file"
  | "file replace"
  | "file splice";

interface EmbeddedOutcome {
  readonly status: number;
  readonly request_variant: string;
  readonly body: "empty" | "json" | "plain_text" | "binary";
  readonly schema:
    | "empty"
    | "asset"
    | "api_version"
    | "hash"
    | "stats"
    | "directory_listing"
    | "hosted_content"
    | "site_mutation_receipt"
    | "archive"
    | "undo_mutation_receipt"
    | "expiry_report"
    | "management_status"
    | "file_inventory"
    | "undo_stack"
    | "expiry_site_report"
    | "alias_receipt"
    | "alias_batch_receipt"
    | "allocation_receipt"
    | "allocation_proposal_receipt"
    | "allocation_cancellation_receipt"
    | "file_replace_receipt"
    | "splice_receipt"
    | "error";
  readonly required_headers: readonly string[];
  readonly optional_headers: readonly string[];
  readonly forbidden_headers: readonly string[];
}

interface EmbeddedOperation {
  readonly name: EndpointName;
  readonly method: string;
  readonly head: boolean;
  readonly path: string;
  readonly introduced: string;
  readonly request_headers: readonly string[];
  readonly response_headers: readonly string[];
  readonly outcomes_exact: boolean;
  readonly success_outcomes: readonly EmbeddedOutcome[];
  readonly error_outcomes: readonly EmbeddedOutcome[];
}

interface EmbeddedContractFixture {
  readonly fixture_version: number;
  readonly extension_normalization: readonly {
    readonly input: string;
    readonly output: string | null;
  }[];
  readonly api_version: string;
  readonly absolute_revision: number;
  readonly source_hash: string;
  readonly generator_version: string;
  readonly build: {
    readonly commit: string;
    readonly dirty: boolean;
  };
  readonly response_identity_headers: readonly string[];
  readonly splice: {
    readonly media_type: string;
    readonly header_max_bytes: number;
    readonly header_max_descriptors: number;
    readonly frame_max_descriptors: number;
    readonly frame_max_metadata_bytes: number;
  };
  readonly operations: readonly EmbeddedOperation[];
}

const CONTRACT_FIXTURE_JSON: string = "{CONTRACT_FIXTURE_JSON}";
const CONTRACT_FIXTURE: EmbeddedContractFixture = JSON.parse(
  CONTRACT_FIXTURE_JSON,
) as EmbeddedContractFixture;

export interface SymbolClientOptions {
  readonly origin?: string | URL;
  readonly token?: string;
  readonly creatorClaim?: string;
  readonly fetch?: typeof globalThis.fetch;
  readonly retryPolicy?: RetryPolicy;
}

export interface RequestOptions {
  readonly token?: string;
  readonly signal?: AbortSignal;
  readonly headers?: HeadersInit;
}

export interface CachedRequestOptions extends RequestOptions {
  readonly ifNoneMatch?: string;
}

export interface MutationOptions extends RequestOptions {
  readonly ifMatch?: string;
  readonly idempotencyKey?: string;
}

export interface PublishOptions extends MutationOptions {
  readonly mediaType?: MediaType | string;
  readonly filename?: string;
  readonly unpack?: boolean;
  readonly managed?: boolean;
}

export interface SitePutOptions extends RequestOptions {
  readonly ifMatch?: string;
  readonly mediaType?: MediaType | string;
  readonly filename?: string;
  readonly unpack?: boolean;
  readonly managed?: boolean;
}

export interface CopyOptions extends MutationOptions {
  readonly managed?: boolean;
}

export interface ExplicitCopyOptions extends RequestOptions {
  readonly managed?: boolean;
}

export interface FilePutOptions extends RequestOptions {
  readonly ifMatch?: string;
  readonly mediaType?: MediaType | string;
}

export interface FileGetOptions extends RequestOptions {
  readonly range?: string;
  readonly ifRange?: string;
  readonly ifNoneMatch?: string;
}

export interface RetryPolicy {
  readonly maxAttempts: number;
  readonly initialDelayMs: number;
  readonly maximumDelayMs: number;
  readonly backoffFactor: number;
  readonly jitter: "none" | "full";
  readonly honorRetryAfter: boolean;
  readonly retryNetworkErrors: boolean;
  readonly retryStatuses: readonly number[];
}

function frozenRetryPolicy(policy: RetryPolicy): Readonly<RetryPolicy> {
  validateRetryPolicy(policy);
  return Object.freeze({
    maxAttempts: policy.maxAttempts,
    initialDelayMs: policy.initialDelayMs,
    maximumDelayMs: policy.maximumDelayMs,
    backoffFactor: policy.backoffFactor,
    jitter: policy.jitter,
    honorRetryAfter: policy.honorRetryAfter,
    retryNetworkErrors: policy.retryNetworkErrors,
    retryStatuses: Object.freeze(Array.from(policy.retryStatuses)),
  });
}

export const RetryPolicies: Readonly<{
  readonly Disabled: Readonly<RetryPolicy>;
  readonly Default: Readonly<RetryPolicy>;
}> = Object.freeze({
  Disabled: frozenRetryPolicy({
    maxAttempts: 1,
    initialDelayMs: 0,
    maximumDelayMs: 0,
    backoffFactor: 1,
    jitter: "none",
    honorRetryAfter: false,
    retryNetworkErrors: false,
    retryStatuses: [],
  }),
  Default: frozenRetryPolicy({
    maxAttempts: 4,
    initialDelayMs: 250,
    maximumDelayMs: 8_000,
    backoffFactor: 2,
    jitter: "full",
    honorRetryAfter: true,
    retryNetworkErrors: true,
    retryStatuses: [408, 425, 429, 500, 502, 503, 504],
  }),
});

export type MediaParameter = readonly [name: string, value: string];

const MEDIA_TOKEN = /^[!#$%&'*+\-.^_`|~0-9A-Za-z]+$/;

export class MediaType {
  readonly type: string;
  readonly subtype: string;
  readonly parameters: readonly MediaParameter[];

  constructor(type: string, subtype: string, parameters: Iterable<MediaParameter> = []) {
    const normalizedType = normalizedToken(type, "media type");
    const normalizedSubtype = normalizedToken(subtype, "media subtype");
    const names = new Set<string>();
    const normalized: MediaParameter[] = [];
    for (const parameter of parameters) {
      if (!Array.isArray(parameter) || parameter.length !== 2) {
        throw new TypeError("media parameters must be [name, value] tuples");
      }
      const name = normalizedToken(parameter[0], "media parameter name");
      if (names.has(name)) {
        throw new TypeError(`duplicate media parameter: ${name}`);
      }
      names.add(name);
      const value = normalizedParameterValue(name, parameter[1]);
      normalized.push(Object.freeze([name, value]) as MediaParameter);
    }
    normalized.sort((left, right) => (left[0] < right[0] ? -1 : left[0] > right[0] ? 1 : 0));
    this.type = normalizedType;
    this.subtype = normalizedSubtype;
    this.parameters = Object.freeze(normalized);
    Object.freeze(this);
  }

  static parse(value: string): MediaType {
    if (typeof value !== "string") {
      throw new TypeError("media type must be a string");
    }
    let index = 0;
    const skipWhitespace = (): void => {
      while (index < value.length && (value[index] === " " || value[index] === "\t")) {
        index += 1;
      }
    };
    const readToken = (label: string): string => {
      const start = index;
      while (index < value.length && MEDIA_TOKEN.test(value[index])) {
        index += 1;
      }
      if (start === index) {
        throw new TypeError(`invalid ${label}`);
      }
      return value.slice(start, index);
    };
    skipWhitespace();
    const type = readToken("media type");
    if (value[index] !== "/") {
      throw new TypeError("media type must contain one slash");
    }
    index += 1;
    const subtype = readToken("media subtype");
    const parameters: MediaParameter[] = [];
    for (;;) {
      skipWhitespace();
      if (index === value.length) {
        break;
      }
      if (value[index] !== ";") {
        throw new TypeError("invalid media type parameter separator");
      }
      index += 1;
      skipWhitespace();
      const name = readToken("media parameter name");
      skipWhitespace();
      if (value[index] !== "=") {
        throw new TypeError("media parameter requires a value");
      }
      index += 1;
      skipWhitespace();
      let parameterValue: string;
      if (value[index] === '"') {
        index += 1;
        let decoded = "";
        let closed = false;
        while (index < value.length) {
          const character = value[index];
          index += 1;
          if (character === '"') {
            closed = true;
            break;
          }
          if (character === "\\") {
            if (index === value.length) {
              throw new TypeError("unterminated media parameter escape");
            }
            const escaped = value[index];
            index += 1;
            if (!isQuotedCharacter(escaped)) {
              throw new TypeError("invalid media parameter escape");
            }
            decoded += escaped;
          } else {
            if (!isQuotedCharacter(character)) {
              throw new TypeError("invalid quoted media parameter");
            }
            decoded += character;
          }
        }
        if (!closed) {
          throw new TypeError("unterminated quoted media parameter");
        }
        parameterValue = decoded;
      } else {
        parameterValue = readToken("media parameter value");
      }
      parameters.push([name, parameterValue]);
    }
    return new MediaType(type, subtype, parameters);
  }

  static application(subtype: string, parameters: Iterable<MediaParameter> = []): MediaType {
    return new MediaType("application", subtype, parameters);
  }

  static text(subtype: string, charset: string = Charsets.Utf8): MediaType {
    return new MediaType("text", subtype, [["charset", charset]]);
  }

  static image(subtype: string, parameters: Iterable<MediaParameter> = []): MediaType {
    return new MediaType("image", subtype, parameters);
  }

  static audio(subtype: string, parameters: Iterable<MediaParameter> = []): MediaType {
    return new MediaType("audio", subtype, parameters);
  }

  static video(subtype: string, parameters: Iterable<MediaParameter> = []): MediaType {
    return new MediaType("video", subtype, parameters);
  }

  static font(subtype: string, parameters: Iterable<MediaParameter> = []): MediaType {
    return new MediaType("font", subtype, parameters);
  }

  static utf8(type: string, subtype: string): MediaType {
    return new MediaType(type, subtype, [["charset", Charsets.Utf8]]);
  }

  get essence(): string {
    return `${this.type}/${this.subtype}`;
  }

  get structuredSuffix(): string | null {
    const separator = this.subtype.lastIndexOf("+");
    return separator > 0 && separator < this.subtype.length - 1
      ? this.subtype.slice(separator + 1)
      : null;
  }

  get charset(): string | null {
    return this.parameter("charset");
  }

  parameter(name: string): string | null {
    const wanted = normalizedToken(name, "media parameter name");
    const found = this.parameters.find((parameter) => parameter[0] === wanted);
    return found === undefined ? null : found[1];
  }

  withParameter(name: string, value: string): MediaType {
    const normalizedName = normalizedToken(name, "media parameter name");
    const parameters = this.parameters.filter((parameter) => parameter[0] !== normalizedName);
    parameters.push([normalizedName, value]);
    return new MediaType(this.type, this.subtype, parameters);
  }

  withCharset(charset: string = Charsets.Utf8): MediaType {
    return this.withParameter("charset", charset);
  }

  withoutParameter(name: string): MediaType {
    const normalizedName = normalizedToken(name, "media parameter name");
    return new MediaType(
      this.type,
      this.subtype,
      this.parameters.filter((parameter) => parameter[0] !== normalizedName),
    );
  }

  equals(other: MediaType | string): boolean {
    const candidate = typeof other === "string" ? MediaType.parse(other) : other;
    return this.toString() === candidate.toString();
  }

  toString(): string {
    let result = this.essence;
    for (const [name, value] of this.parameters) {
      result += `; ${name}=${serializedParameterValue(value)}`;
    }
    return result;
  }

  [Symbol.toPrimitive](): string {
    return this.toString();
  }
}

export class ContentFormat {
  readonly mediaType: MediaType;
  readonly extensions: readonly string[];
  readonly preferredExtension: string | null;

  constructor(mediaType: MediaType, extensions: readonly string[] = []) {
    if (!(mediaType instanceof MediaType)) {
      throw new TypeError("content format requires a MediaType");
    }
    const normalized = Array.from(new Set(extensions.map(normalizeExtension)));
    this.mediaType = mediaType;
    this.extensions = Object.freeze(normalized);
    this.preferredExtension = normalized.length === 0 ? null : normalized[0];
    Object.freeze(this);
  }
}

export const Charsets: Readonly<{
  readonly Utf8: "utf-8";
  readonly UsAscii: "us-ascii";
  readonly Iso88591: "iso-8859-1";
}> = Object.freeze({
  Utf8: "utf-8",
  UsAscii: "us-ascii",
  Iso88591: "iso-8859-1",
} as const);

export const MediaTypes: Readonly<{
  readonly Binary: MediaType;
  readonly Json: MediaType;
  readonly Text: MediaType;
  readonly Html: MediaType;
  readonly Css: MediaType;
  readonly JavaScript: MediaType;
  readonly Markdown: MediaType;
  readonly Xml: MediaType;
  readonly Pdf: MediaType;
  readonly Png: MediaType;
  readonly Jpeg: MediaType;
  readonly Gif: MediaType;
  readonly WebP: MediaType;
  readonly Svg: MediaType;
  readonly Zip: MediaType;
  readonly Gzip: MediaType;
  readonly Wasm: MediaType;
}> = Object.freeze({
  Binary: MediaType.application("octet-stream"),
  Json: MediaType.application("json"),
  Text: MediaType.text("plain"),
  Html: MediaType.text("html"),
  Css: MediaType.text("css"),
  JavaScript: MediaType.text("javascript"),
  Markdown: MediaType.text("markdown"),
  Xml: MediaType.application("xml"),
  Pdf: MediaType.application("pdf"),
  Png: MediaType.image("png"),
  Jpeg: MediaType.image("jpeg"),
  Gif: MediaType.image("gif"),
  WebP: MediaType.image("webp"),
  Svg: MediaType.image("svg+xml"),
  Zip: MediaType.application("zip"),
  Gzip: MediaType.application("gzip"),
  Wasm: MediaType.application("wasm"),
} as const);

export type MediaTypes = (typeof MediaTypes)[keyof typeof MediaTypes];

export const ContentFormats: Readonly<{
  readonly Binary: ContentFormat;
  readonly Json: ContentFormat;
  readonly Text: ContentFormat;
  readonly Html: ContentFormat;
  readonly Css: ContentFormat;
  readonly JavaScript: ContentFormat;
  readonly Markdown: ContentFormat;
  readonly Xml: ContentFormat;
  readonly Pdf: ContentFormat;
  readonly Png: ContentFormat;
  readonly Jpeg: ContentFormat;
  readonly Gif: ContentFormat;
  readonly WebP: ContentFormat;
  readonly Svg: ContentFormat;
  readonly Zip: ContentFormat;
  readonly Gzip: ContentFormat;
  readonly Wasm: ContentFormat;
}> = Object.freeze({
  Binary: new ContentFormat(MediaTypes.Binary),
  Json: new ContentFormat(MediaTypes.Json, ["json"]),
  Text: new ContentFormat(MediaTypes.Text, ["txt"]),
  Html: new ContentFormat(MediaTypes.Html, ["html", "htm"]),
  Css: new ContentFormat(MediaTypes.Css, ["css"]),
  JavaScript: new ContentFormat(MediaTypes.JavaScript, ["js", "mjs"]),
  Markdown: new ContentFormat(MediaTypes.Markdown, ["md", "markdown"]),
  Xml: new ContentFormat(MediaTypes.Xml, ["xml"]),
  Pdf: new ContentFormat(MediaTypes.Pdf, ["pdf"]),
  Png: new ContentFormat(MediaTypes.Png, ["png"]),
  Jpeg: new ContentFormat(MediaTypes.Jpeg, ["jpg", "jpeg"]),
  Gif: new ContentFormat(MediaTypes.Gif, ["gif"]),
  WebP: new ContentFormat(MediaTypes.WebP, ["webp"]),
  Svg: new ContentFormat(MediaTypes.Svg, ["svg"]),
  Zip: new ContentFormat(MediaTypes.Zip, ["zip"]),
  Gzip: new ContentFormat(MediaTypes.Gzip, ["gz"]),
  Wasm: new ContentFormat(MediaTypes.Wasm, ["wasm"]),
} as const);

export type ContentFormats = (typeof ContentFormats)[keyof typeof ContentFormats];

export function normalizeExtension(extension: string): string {
  if (typeof extension !== "string") {
    throw new TypeError("file extension must be a string");
  }
  const trimmed = extension.replace(/^\.+|\.+$/g, "");
  let normalized = "";
  let separator = false;
  for (const character of trimmed) {
    if (/^[0-9A-Za-z]$/.test(character)) {
      if (separator && normalized.length > 0) {
        normalized += "-";
      }
      normalized += character.toLowerCase();
      separator = false;
    } else {
      separator = true;
    }
  }
  if (normalized.length === 0) {
    throw new TypeError("file extension must contain at least one ASCII alphanumeric");
  }
  return normalized;
}

export interface GeneratedNameParts {
  readonly prefix?: string;
  readonly extension?: string;
  readonly suffix?: string;
}

export function allocatedFileName(hash: string, parts: GeneratedNameParts = {}): string {
  if (!/^[0-9a-f]{64}$/.test(hash)) {
    throw new TypeError("allocated file hash must be 64 lowercase hexadecimal characters");
  }
  const prefix = validatedNameFragment(parts.prefix === undefined ? "" : parts.prefix, "prefix");
  const suffix = validatedNameFragment(parts.suffix === undefined ? "" : parts.suffix, "suffix");
  const extension =
    parts.extension === undefined || parts.extension === ""
      ? ""
      : normalizeExtension(parts.extension);
  return `${prefix}${hash}${suffix}${extension === "" ? "" : `.${extension}`}`;
}

export interface ProposedFileName {
  readonly folder: string;
  readonly defaultName: string;
  readonly hash: TreeHash;
  readonly size: number;
  readonly mediaType: string;
  readonly inferredExtension: string | null;
}

export type AllocationName =
  | GeneratedNameParts
  | ((proposal: ProposedFileName) => string | Promise<string>);

export type Expiry =
  | { readonly mode: "never" }
  | { readonly mode: "relative"; readonly duration: string }
  | { readonly mode: "absolute"; readonly at: Date | string }
  | {
      readonly mode: "decay";
      readonly minAge: string;
      readonly maxAge: string;
      readonly maxSize: number | string;
      readonly power: number;
    };

export interface CreateFileOptions extends MutationOptions {
  readonly mediaType?: MediaType | string;
  readonly name?: AllocationName;
  readonly expiry?: Expiry;
}

export interface ContentMutationOptions extends MutationOptions {
  readonly baseHash: string;
  readonly mediaType?: MediaType | string;
  readonly format?: "auto" | "headers" | "framed";
}

export interface ByteSplice {
  readonly offset: number;
  readonly deleteBytes: number;
  readonly insert: ArrayBuffer | ArrayBufferView | Blob;
}

export interface FileEntry {
  readonly path: string;
  readonly hash: TreeHash;
  readonly size: number;
}

export interface AliasInventoryEntry {
  readonly path: string;
  readonly target: string;
  readonly targetKind: "file" | "directory" | null;
  readonly dangling: boolean;
  readonly resolvedHash: TreeHash | null;
  readonly size: number | null;
}

export interface FileInventory {
  readonly site: string;
  readonly contentRevision: number;
  readonly treeHash: TreeHash;
  readonly etag: EntityTag;
  readonly files: readonly FileEntry[];
  readonly aliases: readonly AliasInventoryEntry[];
}

export type DirectoryEntry =
  | { readonly kind: "builtin"; readonly name: string; readonly files: null; readonly bytes: 0 }
  | { readonly kind: "site"; readonly name: string; readonly files: number; readonly bytes: number }
  | {
      readonly kind: "directory";
      readonly name: string;
      readonly files: number;
      readonly bytes: number;
    }
  | { readonly kind: "file"; readonly name: string; readonly bytes: number }
  | {
      readonly kind: "alias";
      readonly name: string;
      readonly target: string;
      readonly targetKind: "file" | "directory" | null;
      readonly dangling: boolean;
      readonly files: number | null;
      readonly bytes: number | null;
    };

export interface DirectoryListing {
  readonly path: string;
  readonly files: number;
  readonly aliases: number;
  readonly bytes: number;
  readonly entries: readonly DirectoryEntry[];
}

export interface NotModified {
  readonly status: 304;
  readonly etag: EntityTag;
  readonly cacheControl: string;
  readonly response: Response;
}

export interface ListingRedirect {
  readonly status: 307;
  readonly location: URL;
  readonly response: Response;
}

export type CachedDirectoryListing =
  | (DirectoryListing & {
      readonly status: 200;
      readonly etag: EntityTag;
      readonly cacheControl: string;
      readonly response: Response;
    })
  | NotModified
  | ListingRedirect;

export type CachedFileInventory =
  | (FileInventory & {
      readonly status: 200;
      readonly response: Response;
    })
  | NotModified;

export interface SizeDistribution {
  readonly min: number | null;
  readonly p25: number | null;
  readonly median: number | null;
  readonly mean: number | null;
  readonly p75: number | null;
  readonly max: number | null;
  readonly iqr: number | null;
  readonly stddev: number | null;
}

export interface CacheStats {
  readonly hits: number;
  readonly misses: number;
  readonly evictions: number;
}

export interface ReaderStats {
  readonly operations: number;
  readonly waits: number;
  readonly waitMicros: number;
  readonly queryMicros: number;
}

export interface SymbolStats {
  readonly sites: number;
  readonly files: number;
  readonly aliases: number;
  readonly blobs: number;
  readonly bytes: number;
  readonly logicalBytes: number;
  readonly savedBytes: number;
  readonly savedFraction: number;
  readonly fileSizes: SizeDistribution;
  readonly blobSizes: SizeDistribution;
  readonly serving: {
    readonly cache: CacheStats;
    readonly readers: ReaderStats;
  };
}

export type UndoKind =
  | "put"
  | "delete_path"
  | "delete_site"
  | "copy"
  | "move"
  | "expiry"
  | "expire_sweep"
  | "put_file"
  | "allocate"
  | "replace"
  | "splice"
  | "alias";

export interface UndoEntry {
  readonly token: UndoToken;
  readonly kind: UndoKind;
  readonly description: string;
  readonly createdAt: Date;
  readonly expiresAt: Date;
  readonly remainingSeconds: number;
}

export interface UndoStack {
  readonly site: string;
  readonly entries: readonly UndoEntry[];
}

export type ExpiryTarget =
  | { readonly site: string; readonly kind: "site"; readonly path: null }
  | { readonly site: string; readonly kind: "folder" | "file"; readonly path: string };

export type ExpiryPolicy =
  | {
      readonly mode: "relative";
      readonly minAgeSeconds: null;
      readonly maxAgeSeconds: null;
      readonly maxSizeBytes: null;
      readonly power: null;
      readonly retentionSeconds: number;
      readonly expiresAt: Date;
    }
  | {
      readonly mode: "absolute";
      readonly minAgeSeconds: null;
      readonly maxAgeSeconds: null;
      readonly maxSizeBytes: null;
      readonly power: null;
      readonly retentionSeconds: null;
      readonly expiresAt: Date;
    }
  | {
      readonly mode: "decay";
      readonly minAgeSeconds: number;
      readonly maxAgeSeconds: number;
      readonly maxSizeBytes: number;
      readonly power: number;
      readonly retentionSeconds: number;
      readonly expiresAt: Date;
    };

export interface ExpiryCap {
  readonly kind: "site" | "folder" | "file";
  readonly path: string | null;
  readonly expiresAt: Date;
}

export interface ExpiryLimit {
  readonly kind: "site" | "folder" | "file";
  readonly path: string | null;
}

export interface ExpiryReport {
  readonly target: ExpiryTarget;
  readonly size: number;
  readonly refreshedAt: Date | null;
  readonly ownPolicy: ExpiryPolicy | null;
  readonly inheritedCaps: readonly ExpiryCap[];
  readonly effectiveExpiresAt: Date | null;
  readonly remainingSeconds: number | null;
  readonly limitedBy: ExpiryLimit | null;
}

export interface ExpirySiteReport {
  readonly site: string;
  readonly entries: readonly ExpiryReport[];
}

export interface ManagementStatus {
  readonly managed: boolean;
}

export interface UndoReceipt {
  readonly token: UndoToken;
  readonly expiresAt: Date;
}

export interface AliasDefinition {
  readonly path: string;
  readonly target: string;
}

export interface MutationBase {
  readonly idempotencyKey: IdempotencyKey | null;
  readonly replayed: boolean;
  readonly location: URL;
  readonly etag: EntityTag;
  readonly contentRevision: number;
  readonly sanitizedManagementTokens: number;
  readonly sanitizedCreatorClaims: number;
  readonly response: Response;
}

export interface ChangedMutation extends MutationBase {
  readonly changed: true;
  readonly undo: UndoReceipt;
}

export interface UnchangedMutation extends MutationBase {
  readonly changed: false;
  readonly undo: null;
}

export type SiteCreationReceipt = {
  readonly status: 201;
  readonly site: string;
  readonly files: number;
  readonly creatorClaim: CreatorClaim | null;
  readonly managementToken: ManagementToken | null;
} & ChangedMutation;

export type SitePutReceipt = {
  readonly status: 200 | 201;
  readonly site: string;
  readonly files: number;
  readonly creatorClaim: CreatorClaim | null;
  readonly managementToken: ManagementToken | null;
} & (ChangedMutation | UnchangedMutation);

export type FilePutReceipt = {
  readonly status: 200 | 201;
  readonly creatorClaim: CreatorClaim | null;
  readonly managementToken: ManagementToken | null;
} & (ChangedMutation | UnchangedMutation);

export type CopyReceipt = {
  readonly status: 201;
  readonly site: string;
  readonly files: number;
  readonly creatorClaim: CreatorClaim | null;
  readonly managementToken: ManagementToken | null;
} & ChangedMutation;

export type MoveReceipt = { readonly status: 200 } & ChangedMutation;

export interface DeleteReceipt {
  readonly status: 200;
  readonly undo: UndoReceipt;
  readonly response: Response;
}

export interface UndoMutationReceipt {
  readonly status: 200;
  readonly restoredAt: Date;
  readonly response: Response;
}

export type ExpiryMutationReceipt = {
  readonly status: 200;
  readonly report: ExpiryReport;
  readonly response: Response;
} & (
  | { readonly changed: true; readonly undo: UndoReceipt }
  | { readonly changed: false; readonly undo: null }
);

export type AliasReceipt = {
  readonly status: 200 | 201;
  readonly path: string;
  readonly target: string;
  readonly targetKind: "file" | "directory" | null;
  readonly dangling: boolean;
  readonly resolvedHash: string | null;
  readonly size: number | null;
} & (ChangedMutation | UnchangedMutation);

export type AliasBatchReceipt = {
  readonly status: 200 | 201;
  readonly aliases: readonly AliasInventoryEntry[];
} & (ChangedMutation | UnchangedMutation);

export type AllocationReceipt = {
  readonly status: 200 | 201;
  readonly outcome: "created" | "existing";
  readonly site: string;
  readonly path: string;
  readonly name: string;
  readonly url: URL;
  readonly hash: TreeHash;
  readonly size: number;
  readonly blobUrl: URL;
  readonly naming:
    | {
        readonly mode: "generated";
        readonly prefix: string;
        readonly extension: string;
        readonly suffix: string;
      }
    | { readonly mode: "custom" };
} & (ChangedMutation | UnchangedMutation);

type FileReplaceReceiptBase = {
  readonly status: 200;
  readonly oldPath: string;
  readonly newPath: string;
  readonly oldHash: TreeHash;
  readonly newHash: TreeHash;
  readonly size: number;
};

export type FileReplaceReceipt = FileReplaceReceiptBase &
  (
    | (ChangedMutation & {
        readonly outcome: "replaced";
        readonly relocated: false;
      })
    | (ChangedMutation & {
        readonly outcome: "relocated";
        readonly relocated: true;
      })
    | (UnchangedMutation & {
        readonly outcome: "unchanged";
        readonly relocated: false;
      })
  );

export type SpliceReceipt = {
  readonly status: 200;
  readonly oldPath: string;
  readonly newPath: string;
  readonly relocated: boolean;
  readonly oldHash: TreeHash;
  readonly newHash: TreeHash;
  readonly oldSize: number;
  readonly newSize: number;
  readonly splices: number;
} & (ChangedMutation | UnchangedMutation);

export type ManagementClaimReceipt =
  | {
      readonly action: "claim";
      readonly status: 200;
      readonly managed: true;
      readonly replayed: false;
      readonly managementToken: ManagementToken;
      readonly response: Response;
    }
  | {
      readonly action: "claim";
      readonly status: 200;
      readonly managed: true;
      readonly replayed: true;
      readonly managementToken: null;
      readonly response: Response;
    };

export type ManagementRotateReceipt =
  | {
      readonly action: "rotate";
      readonly status: 200;
      readonly managed: true;
      readonly replayed: false;
      readonly managementToken: ManagementToken;
      readonly response: Response;
    }
  | {
      readonly action: "rotate";
      readonly status: 200;
      readonly managed: true;
      readonly replayed: true;
      readonly managementToken: null;
      readonly response: Response;
    };

export interface ManagementReleaseReceipt {
  readonly action: "release";
  readonly status: 200;
  readonly managed: false;
  readonly replayed: false;
  readonly managementToken: null;
  readonly response: Response;
}

export type DisposableResponse = Response & AsyncDisposable;

export class ArchiveDownload implements AsyncDisposable {
  readonly format: "tar" | "tar.gz" | "zip";
  readonly filename: string;
  readonly size: number;
  readonly response: Response;
  readonly undo: UndoReceipt | null;
  private consumed: boolean;
  private disposed: boolean;
  private disposePromise: Promise<void> | null;

  constructor(
    format: "tar" | "tar.gz" | "zip",
    filename: string,
    size: number,
    response: Response,
  ) {
    this.format = format;
    this.filename = filename;
    this.size = size;
    this.response = response;
    trackOwnedResponseBody(response);
    this.undo = null;
    this.consumed = false;
    this.disposed = false;
    this.disposePromise = null;
  }

  async blob(): Promise<Blob> {
    if (this.disposed) {
      throw new Error("archive has been disposed");
    }
    const blob = await this.response.blob();
    this.consumed = true;
    return blob;
  }

  async [Symbol.asyncDispose](): Promise<void> {
    if (this.disposePromise === null) {
      this.disposed = true;
      this.disposePromise = this.consumed
        ? Promise.resolve()
        : cancelOwnedResponseBody(this.response);
    }
    await this.disposePromise;
  }
}

export class ArchivePopReceipt extends ArchiveDownload {
  declare readonly undo: UndoReceipt;

  constructor(
    format: "tar" | "tar.gz" | "zip",
    filename: string,
    size: number,
    response: Response,
    undo: UndoReceipt,
  ) {
    super(format, filename, size, response);
    Object.defineProperty(this, "undo", {
      configurable: false,
      enumerable: true,
      value: undo,
      writable: false,
    });
  }
}

export class SymbolApiError extends Error {
  readonly operation: EndpointName | "raw request";
  readonly status: number;
  readonly body: string;
  readonly idempotencyKey: string | null;
  readonly response: Response;

  constructor(
    name: string,
    operation: EndpointName | "raw request",
    status: number,
    body: string,
    idempotencyKey: string | null,
    response: Response,
  ) {
    super(body.trim() === "" ? `${operation} failed with HTTP ${status}` : body.trim());
    this.name = name;
    this.operation = operation;
    this.status = status;
    this.body = body;
    this.idempotencyKey = idempotencyKey;
    this.response = response;
  }

  static async from(
    operation: EndpointName | "raw request",
    response: Response,
    idempotencyKey: string | null = null,
  ): Promise<SymbolApiError> {
    const preserved = response.clone();
    const body = await response.text();
    return errorForStatus(operation, response.status, body, idempotencyKey, preserved);
  }
}

export class ValidationError extends SymbolApiError {
  declare readonly status: 400;
}

export class UnauthorizedError extends SymbolApiError {
  declare readonly status: 401;
  readonly challenge: string;

  constructor(
    operation: EndpointName | "raw request",
    body: string,
    idempotencyKey: string | null,
    response: Response,
    challenge: string,
  ) {
    super("UnauthorizedError", operation, 401, body, idempotencyKey, response);
    this.challenge = challenge;
  }
}

export class ForbiddenError extends SymbolApiError {
  declare readonly status: 403;
}

export class NotFoundError extends SymbolApiError {
  declare readonly status: 404;
}

export class MethodNotAllowedError extends SymbolApiError {
  declare readonly status: 405;
  readonly allow: readonly string[];

  constructor(
    operation: EndpointName | "raw request",
    body: string,
    idempotencyKey: string | null,
    response: Response,
  ) {
    super("MethodNotAllowedError", operation, 405, body, idempotencyKey, response);
    this.allow = Object.freeze(
      (response.headers.get("Allow") || "")
        .split(",")
        .map((value) => value.trim())
        .filter((value) => value !== ""),
    );
  }
}

export class ConflictError extends SymbolApiError {
  declare readonly status: 409;
}

export class PreconditionFailedError extends SymbolApiError {
  declare readonly status: 412;
  readonly etag: string;
  readonly contentRevision: number;

  constructor(
    operation: EndpointName | "raw request",
    body: string,
    idempotencyKey: string | null,
    response: Response,
    etag: string,
    contentRevision: number,
  ) {
    super("PreconditionFailedError", operation, 412, body, idempotencyKey, response);
    this.etag = etag;
    this.contentRevision = contentRevision;
  }
}

export class PayloadTooLargeError extends SymbolApiError {
  declare readonly status: 413;
}

export class RangeNotSatisfiableError extends SymbolApiError {
  declare readonly status: 416;
  readonly contentRange: string | null;

  constructor(
    operation: EndpointName | "raw request",
    body: string,
    idempotencyKey: string | null,
    response: Response,
  ) {
    super("RangeNotSatisfiableError", operation, 416, body, idempotencyKey, response);
    this.contentRange = response.headers.get("Content-Range");
  }
}

export class ServerError extends SymbolApiError {
  declare readonly status: 500;
}

export class UnexpectedResponseError extends SymbolApiError {}
export class MalformedResponseError extends SymbolApiError {}
export class ApiCompatibilityError extends SymbolApiError {}
export class IncompatibleApiVersionError extends ApiCompatibilityError {}
export class OperationUnavailableError extends ApiCompatibilityError {}
export class ApiIntegrityError extends ApiCompatibilityError {}
export class MissingApiIdentityError extends ApiCompatibilityError {}

type ReadProtocolError = ValidationError | NotFoundError | RangeNotSatisfiableError;
type MutationProtocolError =
  | ValidationError
  | UnauthorizedError
  | ForbiddenError
  | NotFoundError
  | ConflictError
  | PreconditionFailedError
  | PayloadTooLargeError
  | ServerError;

export interface EndpointErrorMap {
  readonly docs: never;
  readonly "unnamed put": MutationProtocolError;
  readonly "docs hash": NotFoundError;
  readonly stats: ServerError;
  readonly installer: never;
  readonly "installer hash": NotFoundError;
  readonly client: never;
  readonly "client hash": NotFoundError;
  readonly "api client asset": never;
  readonly "api client hash": NotFoundError;
  readonly "api documentation": NotFoundError;
  readonly "api version": never;
  readonly "site listing": ServerError;
  readonly "site redirect": ValidationError | NotFoundError;
  readonly "site index": ReadProtocolError;
  readonly "site put": MutationProtocolError;
  readonly "site pop": ValidationError | UnauthorizedError | NotFoundError | ServerError;
  readonly "site copy": ValidationError | NotFoundError | ConflictError | ServerError;
  readonly "site move":
    | ValidationError
    | UnauthorizedError
    | NotFoundError
    | ConflictError
    | ServerError;
  readonly "site undo": UnauthorizedError | NotFoundError | ConflictError | ServerError;
  readonly "site expire": ValidationError | UnauthorizedError | NotFoundError | ServerError;
  readonly "site management":
    | ValidationError
    | UnauthorizedError
    | ForbiddenError
    | NotFoundError
    | ConflictError
    | ServerError;
  readonly "site file": ReadProtocolError;
  readonly "file put": MutationProtocolError;
  readonly "file delete": ValidationError | UnauthorizedError | NotFoundError | ServerError;
  readonly "file expire": ValidationError | UnauthorizedError | NotFoundError | ServerError;
  readonly "archive get": ValidationError | NotFoundError | ServerError;
  readonly "archive pop": ValidationError | UnauthorizedError | NotFoundError | ServerError;
  readonly "files inventory": NotFoundError | ServerError;
  readonly "files subtree": NotFoundError | ServerError;
  readonly "file hash": ValidationError | NotFoundError;
  readonly "undo stack": NotFoundError | ServerError;
  readonly "expiry inventory": NotFoundError | ServerError;
  readonly "expiry target": ValidationError | NotFoundError | ServerError;
  readonly "immutable blob": ReadProtocolError;
  readonly "alias batch": MutationProtocolError;
  readonly "alias file": MutationProtocolError;
  readonly "allocated file": MutationProtocolError;
  readonly "file replace": MutationProtocolError;
  readonly "file splice": MutationProtocolError | RangeNotSatisfiableError;
}

export type EndpointError<Name extends EndpointName> = EndpointErrorMap[Name];
export type EndpointFailure<Name extends EndpointName> =
  | EndpointError<Name>
  | ApiCompatibilityError
  | MalformedResponseError
  | UnexpectedResponseError;

export class BodyNotReplayableError extends Error {
  constructor() {
    super("operation body is not replayable");
    this.name = "BodyNotReplayableError";
  }
}

export class OperationStateError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "OperationStateError";
  }
}

export interface OperationAttempt {
  readonly number: number;
  readonly startedAt: Date;
  readonly finishedAt: Date;
  readonly status: number | null;
  readonly error: unknown | null;
}

type InternalOperationState = "running" | "succeeded" | "failed" | "disposed";
type InternalAttemptExecutor<T> = (
  signal: AbortSignal,
  reportStatus: (status: number) => void,
) => Promise<T>;

interface InternalOperationConfiguration<T> {
  readonly executor: InternalAttemptExecutor<T>;
  readonly idempotencyKey: string | null;
  readonly replayable: () => boolean;
  readonly retryBlocked?: () => boolean;
  readonly retrySafe: boolean;
  readonly retryPolicy: RetryPolicy;
  readonly signal: AbortSignal | null;
  readonly dispose: (() => Promise<void>) | null;
}

export class Operation<T> implements PromiseLike<T>, AsyncDisposable {
  readonly idempotencyKey: string | null;
  readonly signal: AbortSignal;
  private readonly controller: AbortController;
  private readonly executor: InternalAttemptExecutor<T>;
  private readonly replayableState: () => boolean;
  private readonly retryBlocked: () => boolean;
  private readonly retrySafe: boolean;
  private readonly retryPolicy: RetryPolicy;
  private readonly disposeCallback: (() => Promise<void>) | null;
  private readonly history: OperationAttempt[];
  private unlinkSignal: (() => void) | null;
  private disposePromise: Promise<void> | null;
  private current: Promise<T>;
  private state: InternalOperationState;
  private value: T | null;

  constructor(configuration: InternalOperationConfiguration<T>) {
    this.idempotencyKey = configuration.idempotencyKey;
    this.replayableState = configuration.replayable;
    this.retryBlocked = configuration.retryBlocked ?? (() => false);
    this.controller = new AbortController();
    this.signal = this.controller.signal;
    this.executor = configuration.executor;
    this.retrySafe = configuration.retrySafe;
    this.retryPolicy = frozenRetryPolicy(configuration.retryPolicy);
    this.disposeCallback = configuration.dispose;
    this.history = [];
    this.disposePromise = null;
    this.state = "running";
    this.value = null;
    this.unlinkSignal = linkSignal(configuration.signal, this.controller);
    this.current = this.run(this.retryPolicy);
  }

  get attempts(): readonly OperationAttempt[] {
    return Object.freeze(Array.from(this.history));
  }

  get replayable(): boolean {
    return this.replayableState();
  }

  get canRetry(): boolean {
    return (
      this.state === "failed" &&
      this.retrySafe &&
      this.replayable &&
      !this.retryBlocked() &&
      !this.signal.aborted
    );
  }

  then<TResult1 = T, TResult2 = never>(
    onfulfilled?: ((value: T) => TResult1 | PromiseLike<TResult1>) | null,
    onrejected?: ((reason: unknown) => TResult2 | PromiseLike<TResult2>) | null,
  ): PromiseLike<TResult1 | TResult2> {
    return this.current.then(onfulfilled, onrejected);
  }

  retry(policy: RetryPolicy = RetryPolicies.Default): Promise<T> {
    if (this.state === "succeeded") {
      throw new OperationStateError("a successful operation cannot be retried");
    }
    if (this.state === "running") {
      throw new OperationStateError("operation is still running");
    }
    if (this.state === "disposed" || this.signal.aborted) {
      throw new OperationStateError("disposed or aborted operation cannot be retried");
    }
    if (this.retryBlocked()) {
      throw new OperationStateError("operation failure cannot be retried safely");
    }
    if (!this.replayable) {
      throw new BodyNotReplayableError();
    }
    if (!this.retrySafe) {
      throw new OperationStateError("operation is not safe to replay");
    }
    const snapshot = frozenRetryPolicy(policy);
    this.state = "running";
    this.current = this.run(snapshot);
    return this.current;
  }

  abort(reason?: unknown): void {
    if (!this.signal.aborted) {
      this.controller.abort(reason);
    }
  }

  async [Symbol.asyncDispose](): Promise<void> {
    if (this.disposePromise === null) {
      this.state = "disposed";
      this.abort(new OperationStateError("operation disposed"));
      this.unlinkExternalSignal();
      this.disposePromise = this.disposeOnce();
    }
    await this.disposePromise;
  }

  private async disposeOnce(): Promise<void> {
    try {
      try {
        await this.current;
      } catch {
        // Disposal observes and suppresses the in-flight operation failure.
      }
      if (this.value !== null) {
        await disposeOwnedValue(this.value);
      }
      if (this.disposeCallback !== null) {
        await this.disposeCallback();
      }
    } finally {
      this.state = "disposed";
    }
  }

  private unlinkExternalSignal(): void {
    const unlink = this.unlinkSignal;
    this.unlinkSignal = null;
    if (unlink !== null) {
      unlink();
    }
  }

  private async run(policy: RetryPolicy): Promise<T> {
    let attemptInRun = 0;
    for (;;) {
      attemptInRun += 1;
      const number = this.history.length + 1;
      const startedAt = immutableDate(new Date());
      let observedStatus: number | null = null;
      try {
        const value = await this.executor(this.signal, (status) => {
          observedStatus = status;
        });
        const attempt = Object.freeze({
          number,
          startedAt,
          finishedAt: immutableDate(new Date()),
          status: observedStatus === null ? responseStatus(value) : observedStatus,
          error: null,
        });
        this.history.push(attempt);
        this.value = value;
        if (this.state !== "disposed") {
          this.state = "succeeded";
        }
        this.unlinkExternalSignal();
        return value;
      } catch (error) {
        const attempt = Object.freeze({
          number,
          startedAt,
          finishedAt: immutableDate(new Date()),
          status: error instanceof SymbolApiError ? error.status : null,
          error,
        });
        this.history.push(attempt);
        if (
          attemptInRun >= policy.maxAttempts ||
          !this.retrySafe ||
          !this.replayableState() ||
          this.signal.aborted ||
          !retryableError(error, policy)
        ) {
          if (this.state !== "disposed") {
            this.state = "failed";
          }
          this.unlinkExternalSignal();
          throw error;
        }
        const delay = retryDelay(error, policy, attemptInRun - 1);
        try {
          await abortableDelay(delay, this.signal);
        } catch (delayError) {
          if (this.state !== "disposed") {
            this.state = "failed";
          }
          this.unlinkExternalSignal();
          throw delayError;
        }
      }
    }
  }
}

interface ClientState {
  readonly origin: URL;
  readonly token: string | null;
  readonly creatorClaim: string | null;
  readonly fetch: typeof globalThis.fetch;
  readonly retryPolicy: RetryPolicy;
  observed: SymbolApiIdentity | null;
}

const CLIENT_STATES = new WeakMap<SymbolClient, ClientState>();

interface ReplayBody {
  readonly replayable: boolean;
  next(): BodyInit | null;
}

interface RequestPlan<T> {
  readonly endpoint: EndpointName;
  readonly path: string;
  readonly method: string;
  readonly options: RequestOptions;
  readonly headers: Headers;
  readonly body: ReplayBody;
  readonly idempotencyKey: string | null;
  readonly sendIdempotencyKey: boolean;
  readonly retrySafe: boolean;
  readonly successStatuses: readonly number[];
  readonly decode: (response: Response) => Promise<T>;
  readonly dispose: (() => Promise<void>) | null;
}

export type ApiClientAsset =
  | "symbol.ts"
  | "symbol.js"
  | "symbol.global.js"
  | "symbol.d.ts"
  | "symbol.py";
export type ApiManual = "index" | "javascript" | "typescript" | "python" | "shell" | "protocol";

export class SymbolClient {
  constructor(options: SymbolClientOptions = {}) {
    const origin = normalizedOrigin(options.origin);
    const fetchImplementation = options.fetch === undefined ? globalThis.fetch : options.fetch;
    if (typeof fetchImplementation !== "function") {
      throw new TypeError("SymbolClient requires a fetch implementation");
    }
    const retryPolicy =
      options.retryPolicy === undefined
        ? RetryPolicies.Disabled
        : frozenRetryPolicy(options.retryPolicy);
    CLIENT_STATES.set(this, {
      origin,
      token: options.token === undefined ? null : options.token,
      creatorClaim: options.creatorClaim === undefined ? null : options.creatorClaim,
      fetch: fetchImplementation.bind(globalThis),
      retryPolicy,
      observed: null,
    });
  }

  get origin(): URL {
    return new URL(stateFor(this).origin.href);
  }

  get apiIdentity(): SymbolApiIdentity | null {
    const observed = stateFor(this).observed;
    return observed === null ? null : Object.freeze({ ...observed });
  }

  assertExactApi(): SymbolApiIdentity {
    const observed = stateFor(this).observed;
    if (observed === null) {
      throw new OperationStateError("no Symbol response has been observed");
    }
    if (
      compareVersion(observed.apiVersion, API_VERSION_PARTS) !== 0 ||
      observed.absoluteRevision !== API_REVISION ||
      observed.sourceHash !== SOURCE_HASH
    ) {
      throw new Error(
        `expected Symbol API ${API_VERSION} revision ${API_REVISION} ${SOURCE_HASH}, ` +
          `observed ${formatApiVersion(observed.apiVersion)} revision ${observed.absoluteRevision} ` +
          observed.sourceHash,
      );
    }
    return Object.freeze({ ...observed });
  }

  request(path: string, init: RequestInit = {}): Operation<DisposableResponse> {
    const state = stateFor(this);
    const body = replayBody(init.body === undefined ? null : init.body);
    const method = (init.method === undefined ? "GET" : init.method).toUpperCase();
    return new Operation({
      idempotencyKey: null,
      replayable: () => body.replayable,
      retrySafe: method === "GET" || method === "HEAD",
      retryPolicy: state.retryPolicy,
      signal: init.signal === undefined || init.signal === null ? null : init.signal,
      dispose: null,
      executor: async (signal, reportStatus) => {
        let response: Response;
        try {
          response = await state.fetch(new URL(path, state.origin), {
            ...init,
            body: body.next(),
            signal,
          });
        } catch (error) {
          if (signal.aborted) {
            throw signal.reason;
          }
          throw new InternalNetworkRequestError(error);
        }
        reportStatus(response.status);
        observeApiIdentity(this, "raw request", response);
        return ownedResponse(response);
      },
    });
  }

  docs(options: CachedRequestOptions = {}): Operation<CachedTextAsset> {
    return textAssetOperation(this, "docs", "/", options);
  }

  docsHash(options: RequestOptions = {}): Operation<string> {
    return plainTextOperation(this, "docs hash", "/HASH", options);
  }

  installer(options: CachedRequestOptions = {}): Operation<CachedTextAsset> {
    return textAssetOperation(this, "installer", "/install.sh", options);
  }

  installerHash(options: RequestOptions = {}): Operation<string> {
    return plainTextOperation(this, "installer hash", "/install.sh/HASH", options);
  }

  shellClient(options: CachedRequestOptions = {}): Operation<CachedTextAsset> {
    return textAssetOperation(this, "client", "/symbol.sh", options);
  }

  shellClientHash(options: RequestOptions = {}): Operation<string> {
    return plainTextOperation(this, "client hash", "/symbol.sh/HASH", options);
  }

  apiClient(asset: ApiClientAsset, options: CachedRequestOptions = {}): Operation<CachedTextAsset> {
    return textAssetOperation(this, "api client asset", `/${asset}`, options);
  }

  apiClientHash(asset: ApiClientAsset, options: RequestOptions = {}): Operation<string> {
    return plainTextOperation(this, "api client hash", `/${asset}/HASH`, options);
  }

  apiManual(
    manual: ApiManual = "index",
    options: CachedRequestOptions = {},
  ): Operation<CachedTextAsset> {
    return textAssetOperation(this, "api documentation", apiManualPath(manual), options);
  }

  apiVersion(options: CachedRequestOptions = {}): Operation<CachedApiIdentity> {
    const headers = new Headers({ Accept: "application/json" });
    if (options.ifNoneMatch !== undefined) {
      headers.set("If-None-Match", options.ifNoneMatch);
    }
    return jsonOperation(this, {
      endpoint: "api version",
      path: "/API/VERSION",
      method: "GET",
      options,
      headers,
      body: replayBody(null),
      idempotencyKey: null,
      sendIdempotencyKey: false,
      retrySafe: true,
      successStatuses: [200, 304],
      decode: decodeCachedApiVersion,
      dispose: null,
    });
  }

  stats(options: RequestOptions = {}): Operation<SymbolStats> {
    return jsonOperation(this, {
      endpoint: "stats",
      path: "/STATS",
      method: "GET",
      options,
      headers: new Headers(),
      body: replayBody(null),
      idempotencyKey: null,
      sendIdempotencyKey: false,
      retrySafe: true,
      successStatuses: [200],
      decode: decodeStats,
      dispose: null,
    });
  }

  sites(options: CachedRequestOptions = {}): Operation<CachedDirectoryListing> {
    const headers = new Headers({ Accept: "application/json" });
    if (options.ifNoneMatch !== undefined) {
      headers.set("If-None-Match", options.ifNoneMatch);
    }
    return jsonOperation(this, {
      endpoint: "site listing",
      path: "/FILES",
      method: "GET",
      options,
      headers,
      body: replayBody(null),
      idempotencyKey: null,
      sendIdempotencyKey: false,
      retrySafe: true,
      successStatuses: [200, 304],
      decode: decodeCachedDirectoryListing,
      dispose: null,
    });
  }

  create(body: BodyInit, options: PublishOptions = {}): Operation<SiteCreationReceipt> {
    const idempotencyKey = effectiveIdempotencyKey(options.idempotencyKey);
    const headers = publishHeaders(options, stateFor(this).creatorClaim);
    return requestOperation(this, {
      endpoint: "unnamed put",
      path: "/",
      method: "PUT",
      options,
      headers,
      body: replayBody(body),
      idempotencyKey,
      sendIdempotencyKey: true,
      retrySafe: true,
      successStatuses: [201],
      decode: async (response) => decodeSiteCreation(response, idempotencyKey),
      dispose: null,
    });
  }

  site(name: string, options: RequestOptions = {}): SiteClient {
    return new SiteClient(this, validateSiteName(name), options);
  }

  blob(site: string, hash: string, options: FileGetOptions = {}): Operation<DisposableResponse> {
    const path = `/.blob/${encodeSegment(validateSiteName(site))}/${encodeSegment(validateHash(hash))}`;
    return hostedResponseOperation(this, "immutable blob", path, options);
  }
}

export type CachedTextAsset =
  | {
      readonly status: 200;
      readonly body: string;
      readonly contentType: string;
      readonly etag: string;
      readonly cacheControl: string;
      readonly response: Response;
    }
  | {
      readonly status: 304;
      readonly body: null;
      readonly contentType: null;
      readonly etag: string;
      readonly cacheControl: string;
      readonly response: Response;
    };

export type CachedApiIdentity =
  | {
      readonly status: 200;
      readonly identity: SymbolApiIdentity;
      readonly etag: string;
      readonly cacheControl: string;
      readonly response: Response;
    }
  | {
      readonly status: 304;
      readonly identity: null;
      readonly etag: string;
      readonly cacheControl: string;
      readonly response: Response;
    };

export interface SiteRedirect {
  readonly status: 307;
  readonly location: URL;
  readonly response: Response;
}

export class SiteClient {
  readonly name: string;
  readonly url: URL;
  private readonly client: SymbolClient;
  private readonly defaults: RequestOptions;

  constructor(client: SymbolClient, name: string, defaults: RequestOptions = {}) {
    this.client = client;
    this.name = validateSiteName(name);
    this.defaults = defaults;
    this.url = endpointUrl(client, [this.name], true);
  }

  file(path: string): FileClient {
    return new FileClient(this.client, this.name, normalizedPath(path), this.defaults);
  }

  folder(path: string = ""): FolderClient {
    return new FolderClient(this.client, this.name, normalizedPath(path, true), this.defaults);
  }

  redirect(options: RequestOptions = {}): Operation<SiteRedirect> {
    const merged = mergedOptions(this.defaults, options);
    return requestOperation(this.client, {
      endpoint: "site redirect",
      path: encodedPath([this.name]),
      method: "GET",
      options: merged,
      headers: new Headers(),
      body: replayBody(null),
      idempotencyKey: null,
      sendIdempotencyKey: false,
      retrySafe: true,
      successStatuses: [307],
      decode: async (response) => {
        const preserved = response.clone();
        const location = requiredHeader(response, "Location");
        return Object.freeze({
          status: 307 as const,
          location: new URL(location, stateFor(this.client).origin),
          response: preserved,
        });
      },
      dispose: null,
    });
  }

  get(options: FileGetOptions = {}): Operation<DisposableResponse> {
    return hostedResponseOperation(
      this.client,
      "site index",
      encodedPath([this.name], true),
      mergedOptions(this.defaults, options),
    );
  }

  put(body: BodyInit, options: SitePutOptions = {}): Operation<SitePutReceipt> {
    const merged: SitePutOptions = {
      ...mergedOptions(this.defaults, options),
      ifMatch: options.ifMatch,
      mediaType: options.mediaType,
      filename: options.filename,
      unpack: options.unpack,
      managed: options.managed,
    };
    return requestOperation(this.client, {
      endpoint: "site put",
      path: encodedPath([this.name]),
      method: "PUT",
      options: merged,
      headers: publishHeaders(options, stateFor(this.client).creatorClaim),
      body: replayBody(body),
      idempotencyKey: null,
      sendIdempotencyKey: false,
      retrySafe: false,
      successStatuses: [200, 201],
      decode: async (response) => decodeSitePut(response),
      dispose: null,
    });
  }

  files(options?: CachedRequestOptions): Operation<CachedFileInventory>;
  files(path: string, options?: CachedRequestOptions): Operation<CachedDirectoryListing>;
  files(
    pathOrOptions: string | CachedRequestOptions = {},
    options: CachedRequestOptions = {},
  ): Operation<CachedFileInventory | CachedDirectoryListing> {
    if (typeof pathOrOptions === "string") {
      const path = normalizedPath(pathOrOptions, true);
      const merged = mergedOptions(this.defaults, options);
      const headers = new Headers({ Accept: "application/json" });
      if (options.ifNoneMatch !== undefined) {
        headers.set("If-None-Match", options.ifNoneMatch);
      }
      return jsonOperation(this.client, {
        endpoint: "files subtree",
        path: encodedPath([this.name, "FILES", ...pathSegments(path)], true),
        method: "GET",
        options: merged,
        headers,
        body: replayBody(null),
        idempotencyKey: null,
        sendIdempotencyKey: false,
        retrySafe: true,
        successStatuses: [200, 304, 307],
        decode: decodeCachedDirectoryListing,
        dispose: null,
      });
    }
    const merged = mergedOptions(this.defaults, pathOrOptions);
    const headers = new Headers({ Accept: "application/json" });
    if (pathOrOptions.ifNoneMatch !== undefined) {
      headers.set("If-None-Match", pathOrOptions.ifNoneMatch);
    }
    return jsonOperation(this.client, {
      endpoint: "files inventory",
      path: encodedPath([this.name, "FILES"]),
      method: "GET",
      options: merged,
      headers,
      body: replayBody(null),
      idempotencyKey: null,
      sendIdempotencyKey: false,
      retrySafe: true,
      successStatuses: [200, 304],
      decode: decodeCachedFileInventory,
      dispose: null,
    });
  }

  archive(
    format: "tar" | "tar.gz" | "zip" = "tar.gz",
    options: RequestOptions = {},
  ): Operation<ArchiveDownload> {
    const selected = archiveFormat(format);
    const merged = mergedOptions(this.defaults, options);
    return requestOperation(this.client, {
      endpoint: "archive get",
      path: `/${encodeSegment(this.name)}.${selected}`,
      method: "GET",
      options: merged,
      headers: new Headers(),
      body: replayBody(null),
      idempotencyKey: null,
      sendIdempotencyKey: false,
      retrySafe: true,
      successStatuses: [200],
      decode: async (response) => decodeArchive(response, selected),
      dispose: null,
    });
  }

  pop(
    format: "tar" | "tar.gz" | "zip" = "tar.gz",
    options: RequestOptions = {},
  ): Operation<ArchivePopReceipt> {
    const selected = archiveFormat(format);
    const merged = mergedOptions(this.defaults, options);
    return requestOperation(this.client, {
      endpoint: "archive pop",
      path: `/${encodeSegment(this.name)}.${selected}`,
      method: "DELETE",
      options: merged,
      headers: new Headers(),
      body: replayBody(null),
      idempotencyKey: null,
      sendIdempotencyKey: false,
      retrySafe: false,
      successStatuses: [200],
      decode: async (response) => decodeArchivePop(response, selected),
      dispose: null,
    });
  }

  remove(options: RequestOptions = {}): Operation<ArchivePopReceipt> {
    const merged = mergedOptions(this.defaults, options);
    return requestOperation(this.client, {
      endpoint: "site pop",
      path: encodedPath([this.name]),
      method: "DELETE",
      options: merged,
      headers: new Headers(),
      body: replayBody(null),
      idempotencyKey: null,
      sendIdempotencyKey: false,
      retrySafe: false,
      successStatuses: [200],
      decode: async (response) => decodeArchivePop(response, "tar.gz"),
      dispose: null,
    });
  }

  copy(options?: CopyOptions): Operation<CopyReceipt>;
  copy(destination: string, options?: ExplicitCopyOptions): Operation<CopyReceipt>;
  copy(
    destinationOrOptions?: string | CopyOptions,
    explicitOptions: ExplicitCopyOptions = {},
  ): Operation<CopyReceipt> {
    const destination = typeof destinationOrOptions === "string" ? destinationOrOptions : undefined;
    const options =
      typeof destinationOrOptions === "string" ? explicitOptions : (destinationOrOptions ?? {});
    const idempotencyKey =
      destination === undefined
        ? effectiveIdempotencyKey((options as CopyOptions).idempotencyKey)
        : null;
    const headers = creationHeaders(stateFor(this.client).creatorClaim);
    if (destination !== undefined) {
      headers.set("Destination", `/${validateSiteName(destination)}`);
    }
    if (options.managed === true) {
      headers.set("Management-Action", "claim");
    }
    const merged =
      destination === undefined
        ? mergedMutationOptions(this.defaults, options as CopyOptions)
        : mergedOptions(this.defaults, options);
    return requestOperation(this.client, {
      endpoint: "site copy",
      path: encodedPath([this.name]),
      method: "COPY",
      options: merged,
      headers,
      body: replayBody(null),
      idempotencyKey,
      sendIdempotencyKey: destination === undefined,
      retrySafe: destination === undefined,
      successStatuses: [201],
      decode: async (response) => decodeCopy(response, idempotencyKey),
      dispose: null,
    });
  }

  move(destination: string, options: RequestOptions = {}): Operation<MoveReceipt> {
    const headers = new Headers({
      Destination: `/${validateSiteName(destination)}`,
    });
    const merged = mergedOptions(this.defaults, options);
    return requestOperation(this.client, {
      endpoint: "site move",
      path: encodedPath([this.name]),
      method: "MOVE",
      options: merged,
      headers,
      body: replayBody(null),
      idempotencyKey: null,
      sendIdempotencyKey: false,
      retrySafe: false,
      successStatuses: [200],
      decode: async (response) => {
        const mutation = await decodeTextMutation(response, null, true);
        if (!mutation.changed || mutation.undo === null) {
          throw malformed(response, "move response omitted changed undo metadata");
        }
        return Object.freeze({
          status: 200 as const,
          ...mutation,
          changed: true as const,
          undo: mutation.undo,
        });
      },
      dispose: null,
    });
  }

  alias(path: string, target: string, options: MutationOptions = {}): Operation<AliasReceipt> {
    const idempotencyKey = effectiveIdempotencyKey(options.idempotencyKey);
    const normalized = normalizedPath(path);
    const headers = new Headers({ "Alias-Target": target });
    const merged = mergedMutationOptions(this.defaults, options);
    return jsonOperation(this.client, {
      endpoint: "alias file",
      path: encodedPath([this.name, ...pathSegments(normalized)]),
      method: "ALIAS",
      options: merged,
      headers,
      body: replayBody(null),
      idempotencyKey,
      sendIdempotencyKey: true,
      retrySafe: true,
      successStatuses: [200, 201],
      decode: async (response) => decodeAliasReceipt(response, idempotencyKey),
      dispose: null,
    });
  }

  aliases(
    definitions: Iterable<AliasDefinition>,
    options: MutationOptions = {},
  ): Operation<AliasBatchReceipt> {
    const aliases = Array.from(definitions, (definition) => ({
      path: normalizedPath(definition.path),
      target: definition.target,
    }));
    const idempotencyKey = effectiveIdempotencyKey(options.idempotencyKey);
    const headers = new Headers({ "Content-Type": "application/json" });
    const merged = mergedMutationOptions(this.defaults, options);
    return jsonOperation(this.client, {
      endpoint: "alias batch",
      path: encodedPath([this.name], true),
      method: "ALIAS",
      options: merged,
      headers,
      body: replayBody(JSON.stringify({ aliases })),
      idempotencyKey,
      sendIdempotencyKey: true,
      retrySafe: true,
      successStatuses: [200, 201],
      decode: async (response) => decodeAliasBatchReceipt(response, idempotencyKey),
      dispose: null,
    });
  }

  undo(token?: string, options: RequestOptions = {}): Operation<UndoMutationReceipt> {
    const headers = new Headers();
    if (token !== undefined) {
      headers.set("Undo-Token", token);
    }
    const merged = mergedMutationOptions(this.defaults, options);
    return requestOperation(this.client, {
      endpoint: "site undo",
      path: encodedPath([this.name]),
      method: "UNDO",
      options: merged,
      headers,
      body: replayBody(null),
      idempotencyKey: null,
      sendIdempotencyKey: false,
      retrySafe: false,
      successStatuses: [200],
      decode: decodeUndoMutation,
      dispose: null,
    });
  }

  undoStack(options: RequestOptions = {}): Operation<UndoStack> {
    return jsonOperation(this.client, {
      endpoint: "undo stack",
      path: encodedPath([this.name, "UNDO"]),
      method: "GET",
      options: mergedOptions(this.defaults, options),
      headers: new Headers(),
      body: replayBody(null),
      idempotencyKey: null,
      sendIdempotencyKey: false,
      retrySafe: true,
      successStatuses: [200],
      decode: decodeUndoStack,
      dispose: null,
    });
  }

  expiry(options?: RequestOptions): Operation<ExpirySiteReport>;
  expiry(path: string, options?: RequestOptions): Operation<ExpiryReport>;
  expiry(
    pathOrOptions: string | RequestOptions = {},
    options: RequestOptions = {},
  ): Operation<ExpirySiteReport | ExpiryReport> {
    if (typeof pathOrOptions === "string") {
      const path = normalizedPath(pathOrOptions);
      return jsonOperation(this.client, {
        endpoint: "expiry target",
        path: encodedPath([this.name, ...pathSegments(path), "EXPIRES"]),
        method: "GET",
        options: mergedOptions(this.defaults, options),
        headers: new Headers(),
        body: replayBody(null),
        idempotencyKey: null,
        sendIdempotencyKey: false,
        retrySafe: true,
        successStatuses: [200],
        decode: decodeExpiryReport,
        dispose: null,
      });
    }
    return jsonOperation(this.client, {
      endpoint: "expiry inventory",
      path: encodedPath([this.name, "EXPIRES"]),
      method: "GET",
      options: mergedOptions(this.defaults, pathOrOptions),
      headers: new Headers(),
      body: replayBody(null),
      idempotencyKey: null,
      sendIdempotencyKey: false,
      retrySafe: true,
      successStatuses: [200],
      decode: decodeExpirySiteReport,
      dispose: null,
    });
  }

  setExpiry(
    expiry: Expiry,
    path?: string,
    options: RequestOptions = {},
  ): Operation<ExpiryMutationReceipt> {
    const endpoint: EndpointName = path === undefined ? "site expire" : "file expire";
    const segments =
      path === undefined ? [this.name] : [this.name, ...pathSegments(normalizedPath(path))];
    return expiryMutationOperation(
      this.client,
      endpoint,
      encodedPath(segments),
      expiry,
      mergedOptions(this.defaults, options),
    );
  }

  management(): ManagementClient {
    return new ManagementClient(this.client, this.name, this.defaults);
  }
}

export class FolderClient {
  readonly path: string;
  private readonly client: SymbolClient;
  private readonly site: string;
  private readonly defaults: RequestOptions;

  constructor(client: SymbolClient, site: string, path: string, defaults: RequestOptions = {}) {
    this.client = client;
    this.site = validateSiteName(site);
    this.path = normalizedPath(path, true);
    this.defaults = defaults;
  }

  bytes(
    value: ArrayBuffer | ArrayBufferView,
    options: CreateFileOptions = {},
  ): Operation<AllocationReceipt> {
    return this.allocate(bodyBytes(value), ContentFormats.Binary, options);
  }

  text(value: string, options: CreateFileOptions = {}): Operation<AllocationReceipt> {
    return this.allocate(value, ContentFormats.Text, options);
  }

  json<T>(value: T, options: CreateFileOptions = {}): Operation<AllocationReceipt> {
    return this.allocate(JSON.stringify(value), ContentFormats.Json, options);
  }

  html(value: string, options: CreateFileOptions = {}): Operation<AllocationReceipt> {
    return this.allocate(value, ContentFormats.Html, options);
  }

  blob(value: Blob, options: CreateFileOptions = {}): Operation<AllocationReceipt> {
    const format = formatForMediaType(value.type);
    return this.allocate(value, format, options);
  }

  create(body: BodyInit, options: CreateFileOptions = {}): Operation<AllocationReceipt> {
    const configured =
      options.mediaType === undefined ? mediaTypeForBody(body) : mediaTypeValue(options.mediaType);
    const format = formatForMediaType(configured.toString());
    return this.allocate(body, format, options);
  }

  private allocate(
    body: BodyInit,
    inferredFormat: ContentFormat,
    options: CreateFileOptions,
  ): Operation<AllocationReceipt> {
    const merged = mergedMutationOptions(this.defaults, options);
    const mediaType =
      options.mediaType === undefined
        ? inferredFormat.mediaType
        : mediaTypeValue(options.mediaType);
    const name = options.name;
    if (typeof name === "function") {
      return customAllocation(
        this.client,
        this.site,
        this.path,
        body,
        mediaType,
        inferredFormat.preferredExtension,
        name,
        merged,
        options,
      );
    }
    const naming = name === undefined ? {} : name;
    const extension =
      naming.extension === undefined
        ? inferredFormat.preferredExtension
        : naming.extension === ""
          ? null
          : normalizeExtension(naming.extension);
    const headers = allocationHeaders(mediaType, naming, extension, options.expiry);
    const idempotencyKey = effectiveIdempotencyKey(options.idempotencyKey);
    return jsonOperation(this.client, {
      endpoint: "allocated file",
      path: allocationPath(this.site, this.path),
      method: "POST",
      options: merged,
      headers,
      body: replayBody(body),
      idempotencyKey,
      sendIdempotencyKey: true,
      retrySafe: true,
      successStatuses: [200, 201],
      decode: async (response) => decodeAllocationReceipt(response, idempotencyKey),
      dispose: null,
    });
  }
}

export class FileClient {
  readonly path: string;
  readonly url: URL;
  private readonly client: SymbolClient;
  private readonly site: string;
  private readonly defaults: RequestOptions;

  constructor(client: SymbolClient, site: string, path: string, defaults: RequestOptions = {}) {
    this.client = client;
    this.site = validateSiteName(site);
    this.path = normalizedPath(path);
    this.defaults = defaults;
    this.url = endpointUrl(client, [this.site, ...pathSegments(this.path)]);
  }

  get(options: FileGetOptions = {}): Operation<DisposableResponse> {
    return hostedResponseOperation(
      this.client,
      "site file",
      encodedPath([this.site, ...pathSegments(this.path)]),
      mergedOptions(this.defaults, options),
    );
  }

  text(options: RequestOptions = {}): Operation<string> {
    return hostedBodyOperation(
      this.client,
      "site file",
      encodedPath([this.site, ...pathSegments(this.path)]),
      mergedOptions(this.defaults, options),
      (response) => response.text(),
    );
  }

  json<T>(options: RequestOptions = {}): Operation<T> {
    return hostedBodyOperation(
      this.client,
      "site file",
      encodedPath([this.site, ...pathSegments(this.path)]),
      mergedOptions(this.defaults, options),
      async (response) => JSON.parse(await response.text()) as T,
    );
  }

  put(body: BodyInit, options: FilePutOptions = {}): Operation<FilePutReceipt> {
    const merged: FilePutOptions = {
      ...mergedOptions(this.defaults, options),
      ifMatch: options.ifMatch,
      mediaType: options.mediaType,
    };
    const headers = new Headers();
    if (options.mediaType !== undefined) {
      headers.set("Content-Type", mediaTypeValue(options.mediaType).toString());
    } else if (body instanceof Blob && body.type !== "") {
      headers.set("Content-Type", MediaType.parse(body.type).toString());
    }
    return requestOperation(this.client, {
      endpoint: "file put",
      path: encodedPath([this.site, ...pathSegments(this.path)]),
      method: "PUT",
      options: merged,
      headers,
      body: replayBody(body),
      idempotencyKey: null,
      sendIdempotencyKey: false,
      retrySafe: false,
      successStatuses: [200, 201],
      decode: async (response) => decodeFilePut(response),
      dispose: null,
    });
  }

  putJson<T>(value: T, options: FilePutOptions = {}): Operation<FilePutReceipt> {
    const merged: FilePutOptions = {
      ...options,
      mediaType: options.mediaType === undefined ? MediaTypes.Json : options.mediaType,
    };
    return this.put(JSON.stringify(value), merged);
  }

  replace(body: BodyInit, options: ContentMutationOptions): Operation<FileReplaceReceipt> {
    const idempotencyKey = effectiveIdempotencyKey(options.idempotencyKey);
    const headers = new Headers({
      "If-Content-Match": options.baseHash,
    });
    if (options.mediaType !== undefined) {
      headers.set("Content-Type", mediaTypeValue(options.mediaType).toString());
    } else if (body instanceof Blob && body.type !== "") {
      headers.set("Content-Type", MediaType.parse(body.type).toString());
    } else if (options.headers !== undefined) {
      const supplied = new Headers(options.headers);
      const contentType = supplied.get("Content-Type");
      if (contentType !== null) {
        headers.set("Content-Type", contentType);
      }
    }
    return jsonOperation(this.client, {
      endpoint: "file replace",
      path: encodedPath([this.site, ...pathSegments(this.path)]),
      method: "REPLACE",
      options: mergedOptions(this.defaults, options),
      headers,
      body: replayBody(body),
      idempotencyKey,
      sendIdempotencyKey: true,
      retrySafe: true,
      successStatuses: [200],
      decode: async (response) => decodeFileReplaceReceipt(response, idempotencyKey),
      dispose: null,
    });
  }

  splice(change: ByteSplice, options: ContentMutationOptions): Operation<SpliceReceipt> {
    return this.patch([change], options);
  }

  patch(changes: Iterable<ByteSplice>, options: ContentMutationOptions): Operation<SpliceReceipt> {
    const descriptors = snapshotSplices(changes);
    const expectedSizeDelta = descriptors.reduce(
      (delta, descriptor) => delta + descriptor.insertBytes - descriptor.deleteBytes,
      0,
    );
    if (!Number.isSafeInteger(expectedSizeDelta)) {
      throw new RangeError("splice size delta exceeds JavaScript's safe integer range");
    }
    const idempotencyKey = effectiveIdempotencyKey(options.idempotencyKey);
    let prepared: Promise<PreparedSplices> | null = null;
    const state = stateFor(this.client);
    const merged = mergedMutationOptions(this.defaults, options);
    return new Operation({
      idempotencyKey,
      replayable: () => true,
      retrySafe: true,
      retryPolicy: state.retryPolicy,
      signal: merged.signal === undefined ? null : merged.signal,
      dispose: null,
      executor: async (signal, reportStatus) => {
        if (prepared === null) {
          prepared = prepareSplices(
            descriptors,
            options.format === undefined ? "auto" : options.format,
          );
        }
        const splice = await prepared;
        const headers = new Headers(splice.headers);
        headers.set("If-Content-Match", options.baseHash);
        const response = await executeRequest(
          this.client,
          {
            endpoint: "file splice",
            path: encodedPath([this.site, ...pathSegments(this.path)]),
            method: "PATCH",
            options: merged,
            headers,
            body: replayBody(splice.body),
            idempotencyKey,
            sendIdempotencyKey: true,
            retrySafe: true,
            successStatuses: [200],
            decode: async (value) =>
              decodeSpliceReceipt(value, idempotencyKey, descriptors.length, expectedSizeDelta),
            dispose: null,
          },
          signal,
          reportStatus,
        );
        return response;
      },
    });
  }

  remove(options: RequestOptions = {}): Operation<DeleteReceipt> {
    const merged = mergedOptions(this.defaults, options);
    return requestOperation(this.client, {
      endpoint: "file delete",
      path: encodedPath([this.site, ...pathSegments(this.path)]),
      method: "DELETE",
      options: merged,
      headers: new Headers(),
      body: replayBody(null),
      idempotencyKey: null,
      sendIdempotencyKey: false,
      retrySafe: false,
      successStatuses: [200],
      decode: decodeDeleteReceipt,
      dispose: null,
    });
  }

  hash(options: RequestOptions = {}): Operation<string> {
    return plainTextOperation(
      this.client,
      "file hash",
      encodedPath([this.site, ...pathSegments(this.path), "HASH"]),
      mergedOptions(this.defaults, options),
    );
  }

  expiry(options: RequestOptions = {}): Operation<ExpiryReport> {
    return jsonOperation(this.client, {
      endpoint: "expiry target",
      path: encodedPath([this.site, ...pathSegments(this.path), "EXPIRES"]),
      method: "GET",
      options: mergedOptions(this.defaults, options),
      headers: new Headers(),
      body: replayBody(null),
      idempotencyKey: null,
      sendIdempotencyKey: false,
      retrySafe: true,
      successStatuses: [200],
      decode: decodeExpiryReport,
      dispose: null,
    });
  }

  setExpiry(expiry: Expiry, options: RequestOptions = {}): Operation<ExpiryMutationReceipt> {
    return expiryMutationOperation(
      this.client,
      "file expire",
      encodedPath([this.site, ...pathSegments(this.path)]),
      expiry,
      mergedOptions(this.defaults, options),
    );
  }
}

export class ManagementClient {
  private readonly client: SymbolClient;
  private readonly site: string;
  private readonly defaults: RequestOptions;

  constructor(client: SymbolClient, site: string, defaults: RequestOptions = {}) {
    this.client = client;
    this.site = validateSiteName(site);
    this.defaults = defaults;
  }

  status(options: RequestOptions = {}): Operation<ManagementStatus> {
    const headers = new Headers({ "Management-Action": "status" });
    return jsonOperation(this.client, {
      endpoint: "site management",
      path: encodedPath([this.site]),
      method: "MANAGE",
      options: mergedOptions(this.defaults, options),
      headers,
      body: replayBody(null),
      idempotencyKey: null,
      sendIdempotencyKey: false,
      retrySafe: true,
      successStatuses: [200],
      decode: decodeManagementStatus,
      dispose: null,
    });
  }

  claim(options: MutationOptions = {}): Operation<ManagementClaimReceipt> {
    return managementMutation(
      this.client,
      this.site,
      "claim",
      mergedMutationOptions(this.defaults, options),
    );
  }

  rotate(options: MutationOptions = {}): Operation<ManagementRotateReceipt> {
    return managementMutation(
      this.client,
      this.site,
      "rotate",
      mergedMutationOptions(this.defaults, options),
    );
  }

  release(options: RequestOptions = {}): Operation<ManagementReleaseReceipt> {
    const headers = new Headers({ "Management-Action": "release" });
    return requestOperation(this.client, {
      endpoint: "site management",
      path: encodedPath([this.site]),
      method: "MANAGE",
      options: mergedOptions(this.defaults, options),
      headers,
      body: replayBody(null),
      idempotencyKey: null,
      sendIdempotencyKey: false,
      retrySafe: false,
      successStatuses: [200],
      decode: async (response) => {
        const preserved = response.clone();
        const status = await decodeManagementStatus(response);
        if (status.managed) {
          throw malformed(response, "management release returned managed=true");
        }
        return Object.freeze({
          action: "release" as const,
          status: 200 as const,
          managed: false as const,
          replayed: false as const,
          managementToken: null,
          response: preserved,
        });
      },
      dispose: null,
    });
  }
}

export class AllocationProposal implements ProposedFileName, AsyncDisposable {
  readonly folder: string;
  readonly defaultName: string;
  readonly hash: string;
  readonly size: number;
  readonly mediaType: string;
  readonly inferredExtension: string | null;
  private readonly cancel: () => Promise<void>;
  private disposed: boolean;

  constructor(value: ProposedFileName, cancel: () => Promise<void>) {
    this.folder = value.folder;
    this.defaultName = value.defaultName;
    this.hash = value.hash;
    this.size = value.size;
    this.mediaType = value.mediaType;
    this.inferredExtension = value.inferredExtension;
    this.cancel = cancel;
    this.disposed = false;
  }

  async [Symbol.asyncDispose](): Promise<void> {
    if (this.disposed) {
      return;
    }
    this.disposed = true;
    await this.cancel();
  }
}

interface ProposalWireReceipt {
  readonly allocationToken: string;
  readonly expiresAt: Date;
  readonly proposal: ProposedFileName;
  readonly idempotencyKey: string;
  readonly replayed: boolean;
}

interface CustomAllocationState {
  proposal: ProposalWireReceipt | null;
  proposedResource: AllocationProposal | null;
  chosenName: string | null;
  finalized: boolean;
  cancelled: boolean;
  callbackFailed: boolean;
}

function customAllocation(
  client: SymbolClient,
  site: string,
  folder: string,
  body: BodyInit,
  mediaType: MediaType,
  inferredExtension: string | null,
  chooseName: (proposal: ProposedFileName) => string | Promise<string>,
  options: MutationOptions,
  createOptions: CreateFileOptions,
): Operation<AllocationReceipt> {
  const logicalKey = effectiveIdempotencyKey(createOptions.idempotencyKey);
  const proposalKey = phaseIdempotencyKey(logicalKey, "propose");
  const finalizeKey = phaseIdempotencyKey(logicalKey, "finalize");
  const cancelKey = phaseIdempotencyKey(logicalKey, "cancel");
  const preparedBody = replayBody(body);
  const state: CustomAllocationState = {
    proposal: null,
    proposedResource: null,
    chosenName: null,
    finalized: false,
    cancelled: false,
    callbackFailed: false,
  };
  const cancel = async (): Promise<void> => {
    const proposal = state.proposal;
    if (proposal === null || state.finalized || state.cancelled) {
      return;
    }
    const headers = new Headers({
      "Allocation-Action": "cancel",
      "Allocation-Token": proposal.allocationToken,
    });
    const plan: RequestPlan<Response> = {
      endpoint: "allocated file",
      path: allocationPath(site, folder),
      method: "POST",
      options: { ...options, signal: undefined },
      headers,
      body: replayBody(null),
      idempotencyKey: cancelKey,
      sendIdempotencyKey: true,
      retrySafe: true,
      successStatuses: [200],
      decode: async (response) => {
        await decodeCancellationReceipt(response, cancelKey, proposal.allocationToken);
        return response;
      },
      dispose: null,
    };
    const cancellation = new Operation({
      idempotencyKey: logicalKey,
      replayable: () => true,
      retrySafe: true,
      retryPolicy: RetryPolicies.Default,
      signal: null,
      dispose: null,
      executor: (signal, reportStatus) => executeRequest(client, plan, signal, reportStatus),
    });
    await Promise.resolve(cancellation);
    state.cancelled = true;
  };
  const clientState = stateFor(client);
  return new Operation({
    idempotencyKey: logicalKey,
    replayable: () => state.proposal !== null || preparedBody.replayable,
    retryBlocked: () => state.callbackFailed,
    retrySafe: true,
    retryPolicy: clientState.retryPolicy,
    signal: options.signal === undefined ? null : options.signal,
    dispose: cancel,
    executor: async (signal, reportStatus) => {
      if (state.proposal === null) {
        const headers = new Headers({
          "Allocation-Action": "propose",
          "Content-Type": mediaType.toString(),
        });
        if (inferredExtension !== null) {
          headers.set("File-Extension", inferredExtension);
        }
        addExpiryHeaders(headers, createOptions.expiry);
        state.proposal = await executeRequest(
          client,
          {
            endpoint: "allocated file",
            path: allocationPath(site, folder),
            method: "POST",
            options,
            headers,
            body: preparedBody,
            idempotencyKey: proposalKey,
            sendIdempotencyKey: true,
            retrySafe: true,
            successStatuses: [202],
            decode: async (response) => decodeProposalReceipt(response, proposalKey),
            dispose: null,
          },
          signal,
          reportStatus,
        );
      }
      const proposal = state.proposal;
      if (state.chosenName === null) {
        state.proposedResource = new AllocationProposal(proposal.proposal, cancel);
        try {
          state.chosenName = validateBasename(await chooseName(state.proposedResource));
        } catch (error) {
          state.callbackFailed = true;
          try {
            await cancel();
          } catch {
            // Cancellation is best-effort; preserve the callback's original failure.
          }
          throw error;
        }
      }
      const headers = new Headers({
        "Allocation-Action": "finalize",
        "Allocation-Token": proposal.allocationToken,
        "File-Name": state.chosenName,
      });
      const receipt = await executeRequest(
        client,
        {
          endpoint: "allocated file",
          path: allocationPath(site, folder),
          method: "POST",
          options,
          headers,
          body: replayBody(null),
          idempotencyKey: finalizeKey,
          sendIdempotencyKey: true,
          retrySafe: true,
          successStatuses: [200, 201],
          decode: async (response) =>
            decodeAllocationReceipt(
              response,
              finalizeKey,
              logicalKey,
              proposal.proposal,
              state.chosenName,
            ),
          dispose: null,
        },
        signal,
        reportStatus,
      );
      state.finalized = true;
      return receipt;
    },
  });
}

function requestOperation<T>(client: SymbolClient, plan: RequestPlan<T>): Operation<T> {
  const state = stateFor(client);
  return new Operation({
    idempotencyKey: plan.idempotencyKey,
    replayable: () => plan.body.replayable,
    retrySafe: plan.retrySafe,
    retryPolicy: state.retryPolicy,
    signal: plan.options.signal === undefined ? null : plan.options.signal,
    dispose: plan.dispose,
    executor: (signal, reportStatus) => executeRequest(client, plan, signal, reportStatus),
  });
}

function jsonOperation<T>(client: SymbolClient, plan: RequestPlan<T>): Operation<T> {
  return requestOperation(client, plan);
}

async function executeRequest<T>(
  client: SymbolClient,
  plan: RequestPlan<T>,
  signal: AbortSignal,
  reportStatus: ((status: number) => void) | null = null,
): Promise<T> {
  const state = stateFor(client);
  const contract = contractOperation(plan.endpoint);
  if (contract.method !== plan.method) {
    throw new Error(`generated operation method drift for ${plan.endpoint}`);
  }
  const headers = mergedHeaders(plan.options.headers, plan.headers);
  if (contract.request_headers.some((name) => name.toLowerCase() === "authorization")) {
    addAuthorization(headers, state, plan.options);
  }
  if (
    contract.request_headers.some((name) => name.toLowerCase() === "if-match") &&
    plan.options instanceof Object &&
    "ifMatch" in plan.options
  ) {
    const mutation = plan.options as MutationOptions;
    if (mutation.ifMatch !== undefined) {
      headers.set("If-Match", mutation.ifMatch);
    }
  }
  if (plan.sendIdempotencyKey && plan.idempotencyKey !== null) {
    headers.set("Idempotency-Key", plan.idempotencyKey);
  }
  let response: Response;
  try {
    const requestBody = plan.body.next();
    const init: RequestInit & { duplex?: "half" } = {
      method: plan.method,
      headers,
      body: requestBody,
      redirect: "manual",
      signal,
    };
    if (typeof ReadableStream !== "undefined" && requestBody instanceof ReadableStream) {
      init.duplex = "half";
    }
    response = await state.fetch(new URL(plan.path, state.origin), init);
  } catch (error) {
    if (signal.aborted) {
      throw signal.reason;
    }
    throw new InternalNetworkRequestError(error);
  }
  if (reportStatus !== null) {
    reportStatus(response.status);
  }
  observeApiIdentity(client, plan.endpoint, response);
  if (!plan.successStatuses.includes(response.status)) {
    if (
      !contract.error_outcomes.some((outcome) => outcome.status === response.status) &&
      !contract.success_outcomes.some((outcome) => outcome.status === response.status)
    ) {
      const preserved = response.clone();
      const body = await response.text();
      throw new UnexpectedResponseError(
        "UnexpectedResponseError",
        plan.endpoint,
        response.status,
        body,
        plan.idempotencyKey,
        preserved,
      );
    }
    throw await SymbolApiError.from(plan.endpoint, response, plan.idempotencyKey);
  }
  try {
    return await plan.decode(response);
  } catch (error) {
    if (error instanceof SymbolApiError) {
      throw error;
    }
    const preserved = response.bodyUsed
      ? new Response(null, {
          status: response.status,
          headers: response.headers,
        })
      : response.clone();
    throw new MalformedResponseError(
      "MalformedResponseError",
      plan.endpoint,
      response.status,
      error instanceof Error ? error.message : String(error),
      plan.idempotencyKey,
      preserved,
    );
  }
}

function hostedResponseOperation(
  client: SymbolClient,
  endpoint: "site index" | "site file" | "immutable blob",
  path: string,
  options: FileGetOptions,
): Operation<DisposableResponse> {
  const headers = fileGetHeaders(options);
  const successes =
    endpoint === "site index"
      ? [200, 206, 304, 307]
      : endpoint === "site file"
        ? [200, 206, 304, 307]
        : [200, 206, 304];
  return requestOperation(client, {
    endpoint,
    path,
    method: "GET",
    options,
    headers,
    body: replayBody(null),
    idempotencyKey: null,
    sendIdempotencyKey: false,
    retrySafe: true,
    successStatuses: successes,
    decode: async (response) => {
      validateHostedResponse(response);
      return ownedResponse(response);
    },
    dispose: null,
  });
}

function hostedBodyOperation<T>(
  client: SymbolClient,
  endpoint: "site file",
  path: string,
  options: RequestOptions,
  decode: (response: Response) => Promise<T>,
): Operation<T> {
  return requestOperation(client, {
    endpoint,
    path,
    method: "GET",
    options,
    headers: new Headers(),
    body: replayBody(null),
    idempotencyKey: null,
    sendIdempotencyKey: false,
    retrySafe: true,
    successStatuses: [200],
    decode,
    dispose: null,
  });
}

function textAssetOperation(
  client: SymbolClient,
  endpoint: "docs" | "installer" | "client" | "api client asset" | "api documentation",
  path: string,
  options: CachedRequestOptions,
): Operation<CachedTextAsset> {
  const headers = new Headers();
  if (options.ifNoneMatch !== undefined) {
    headers.set("If-None-Match", options.ifNoneMatch);
  }
  return requestOperation(client, {
    endpoint,
    path,
    method: "GET",
    options,
    headers,
    body: replayBody(null),
    idempotencyKey: null,
    sendIdempotencyKey: false,
    retrySafe: true,
    successStatuses: [200, 304],
    decode: async (response) => {
      const preserved = response.clone();
      const etag = quotedHashHeader(response, "ETag");
      requireCacheControl(response);
      const cacheControl = requiredHeader(response, "Cache-Control");
      if (response.status === 304) {
        forbidHeaders(response, ["Content-Type", "Content-Length"]);
        return Object.freeze({
          status: 304 as const,
          body: null,
          contentType: null,
          etag,
          cacheControl,
          response: preserved,
        });
      }
      MediaType.parse(requiredHeader(response, "Content-Type"));
      return Object.freeze({
        status: 200 as const,
        body: await response.text(),
        contentType: requiredHeader(response, "Content-Type"),
        etag,
        cacheControl,
        response: preserved,
      });
    },
    dispose: null,
  });
}

async function decodeCachedApiVersion(response: Response): Promise<CachedApiIdentity> {
  const preserved = response.clone();
  const etag = quotedHashHeader(response, "ETag");
  requireCacheControl(response);
  const cacheControl = requiredHeader(response, "Cache-Control");
  if (response.status === 304) {
    forbidHeaders(response, ["Content-Type", "Content-Length"]);
    return Object.freeze({
      status: 304,
      identity: null,
      etag,
      cacheControl,
      response: preserved,
    });
  }
  const value = exactRecord(
    await jsonValue(response),
    ["api_version", "absolute_revision", "source_hash"],
    "API version",
  );
  const sourceHash = stringValue(value.source_hash, "API version.source_hash");
  if (!isSourceHash(sourceHash)) {
    throw new TypeError("API version.source_hash must be 64 lowercase hexadecimal characters");
  }
  const identity = Object.freeze({
    apiVersion: apiVersion(stringValue(value.api_version, "API version.api_version")),
    absoluteRevision: unsigned(value.absolute_revision, "API version.absolute_revision"),
    sourceHash: blake3(sourceHash),
  });
  return Object.freeze({
    status: 200,
    identity,
    etag,
    cacheControl,
    response: preserved,
  });
}

function plainTextOperation(
  client: SymbolClient,
  endpoint: "docs hash" | "installer hash" | "client hash" | "api client hash" | "file hash",
  path: string,
  options: RequestOptions,
): Operation<string> {
  return requestOperation(client, {
    endpoint,
    path,
    method: "GET",
    options,
    headers: new Headers(),
    body: replayBody(null),
    idempotencyKey: null,
    sendIdempotencyKey: false,
    retrySafe: true,
    successStatuses: [200],
    decode: async (response) => {
      requireMediaType(response, "text/plain");
      const value = (await response.text()).trimEnd();
      if (!/^[0-9a-f]{64}$/.test(value)) {
        throw malformed(response, "hash must be 64 lowercase hexadecimal characters");
      }
      return value;
    },
    dispose: null,
  });
}

function expiryMutationOperation(
  client: SymbolClient,
  endpoint: "site expire" | "file expire",
  path: string,
  expiry: Expiry,
  options: RequestOptions,
): Operation<ExpiryMutationReceipt> {
  const headers = new Headers();
  addExpiryHeaders(headers, expiry);
  return jsonOperation(client, {
    endpoint,
    path,
    method: "EXPIRE",
    options,
    headers,
    body: replayBody(null),
    idempotencyKey: null,
    sendIdempotencyKey: false,
    retrySafe: false,
    successStatuses: [200],
    decode: async (response) => {
      const preserved = response.clone();
      const report = await decodeExpiryReport(response);
      const undo = optionalUndoHeaders(response);
      return Object.freeze({
        status: 200 as const,
        report,
        changed: undo !== null,
        undo,
        response: preserved,
      }) as ExpiryMutationReceipt;
    },
    dispose: null,
  });
}

function managementMutation(
  client: SymbolClient,
  site: string,
  action: "claim",
  options: MutationOptions,
): Operation<ManagementClaimReceipt>;
function managementMutation(
  client: SymbolClient,
  site: string,
  action: "rotate",
  options: MutationOptions,
): Operation<ManagementRotateReceipt>;
function managementMutation(
  client: SymbolClient,
  site: string,
  action: "claim" | "rotate",
  options: MutationOptions,
): Operation<ManagementClaimReceipt | ManagementRotateReceipt> {
  const idempotencyKey = effectiveIdempotencyKey(options.idempotencyKey);
  const headers = new Headers({ "Management-Action": action });
  const creatorClaim = stateFor(client).creatorClaim;
  if (creatorClaim !== null) {
    headers.set("Creator-Claim", creatorClaim);
  }
  return requestOperation(client, {
    endpoint: "site management",
    path: encodedPath([site]),
    method: "MANAGE",
    options,
    headers,
    body: replayBody(null),
    idempotencyKey,
    sendIdempotencyKey: true,
    retrySafe: true,
    successStatuses: [200],
    decode: async (response) => {
      const preserved = response.clone();
      const status = await decodeManagementStatus(response);
      if (!status.managed) {
        throw malformed(response, `management ${action} returned managed=false`);
      }
      const replayed = response.headers.get("Idempotency-Replayed") === "true";
      const token = response.headers.get("Management-Token");
      if ((!replayed && token === null) || (replayed && token !== null)) {
        throw malformed(response, "management token/replay headers are inconsistent");
      }
      if (replayed) {
        return Object.freeze({
          action,
          status: 200 as const,
          managed: true as const,
          replayed: true as const,
          managementToken: null,
          response: preserved,
        });
      }
      return Object.freeze({
        action,
        status: 200 as const,
        managed: true as const,
        replayed: false as const,
        managementToken: token as string,
        response: preserved,
      });
    },
    dispose: null,
  });
}

function observeApiIdentity(
  client: SymbolClient,
  endpoint: EndpointName | "raw request",
  response: Response,
): void {
  const version = response.headers.get("Symbol-API-Version");
  const revisionText = response.headers.get("Symbol-API-Revision");
  const sourceHash = response.headers.get("Symbol-API-Source-Hash");
  if (version === null || revisionText === null || sourceHash === null) {
    throw new MissingApiIdentityError(
      "MissingApiIdentityError",
      endpoint,
      response.status,
      "response omitted Symbol API identity headers",
      null,
      response.clone(),
    );
  }
  const revision = Number(revisionText);
  if (!Number.isSafeInteger(revision) || revision <= 0 || !isSourceHash(sourceHash)) {
    throw new MissingApiIdentityError(
      "MissingApiIdentityError",
      endpoint,
      response.status,
      "response contained invalid Symbol API identity headers",
      null,
      response.clone(),
    );
  }
  let serverVersion: ApiVersion;
  try {
    serverVersion = apiVersion(version);
  } catch (error) {
    throw new MissingApiIdentityError(
      "MissingApiIdentityError",
      endpoint,
      response.status,
      error instanceof Error ? error.message : "invalid Symbol API version",
      null,
      response.clone(),
    );
  }
  if (serverVersion[0] !== API_VERSION_PARTS[0]) {
    throw new IncompatibleApiVersionError(
      "IncompatibleApiVersionError",
      endpoint,
      response.status,
      `client API ${API_VERSION} is incompatible with server API ${version}`,
      null,
      response.clone(),
    );
  }
  if (endpoint !== "raw request") {
    const introduced = apiVersion(contractOperation(endpoint).introduced);
    if (compareVersion(serverVersion, introduced) < 0) {
      throw new OperationUnavailableError(
        "OperationUnavailableError",
        endpoint,
        response.status,
        `${endpoint} requires API ${contractOperation(endpoint).introduced}; server is ${version}`,
        null,
        response.clone(),
      );
    }
  }
  if (version === API_VERSION && revision === API_REVISION && sourceHash !== SOURCE_HASH) {
    throw new ApiIntegrityError(
      "ApiIntegrityError",
      endpoint,
      response.status,
      "identical Symbol API version/revision reported a different source hash",
      null,
      response.clone(),
    );
  }
  const state = stateFor(client);
  const identity = Object.freeze({
    apiVersion: serverVersion,
    absoluteRevision: revision,
    sourceHash: blake3(sourceHash),
  });
  if (state.observed !== null) {
    const versionOrder = compareVersion(identity.apiVersion, state.observed.apiVersion);
    const revisionOrder = identity.absoluteRevision - state.observed.absoluteRevision;
    const identicalIdentity = versionOrder === 0 && revisionOrder === 0;
    if (
      versionOrder < 0 ||
      revisionOrder < 0 ||
      (versionOrder > 0 && revisionOrder <= 0) ||
      (identicalIdentity && state.observed.sourceHash !== identity.sourceHash)
    ) {
      throw new ApiIntegrityError(
        "ApiIntegrityError",
        endpoint,
        response.status,
        "Symbol API identity rolled back or changed inconsistently during one client run",
        null,
        response.clone(),
      );
    }
  }
  state.observed = identity;
}

function apiArtifact(value: string): ApiArtifact {
  switch (value) {
    case "symbol.ts":
    case "symbol.js":
    case "symbol.global.js":
    case "symbol.d.ts":
      return value;
    default:
      throw new TypeError(`invalid generated API artifact: ${value}`);
  }
}

function apiVersion(value: string): ApiVersion {
  const match = /^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$/.exec(value);
  if (match === null) {
    throw new TypeError(`invalid Symbol API version: ${value}`);
  }
  const version = [Number(match[1]), Number(match[2]), Number(match[3])] as const;
  if (!version.every(Number.isSafeInteger)) {
    throw new TypeError(`Symbol API version is outside JavaScript's safe integer range: ${value}`);
  }
  return Object.freeze(version);
}

function formatApiVersion(version: ApiVersion): string {
  return version.join(".");
}

function compareVersion(left: ApiVersion, right: ApiVersion): number {
  return left[0] - right[0] || left[1] - right[1] || left[2] - right[2];
}

function blake3(value: string): Blake3 {
  if (!/^[0-9a-f]{64}$/.test(value)) {
    throw new TypeError(`invalid Blake3 hash: ${value}`);
  }
  return value as Blake3;
}

function gitCommit(value: string): GitCommit {
  if (value !== "unknown" && !/^[0-9a-fA-F]{7,64}$/.test(value)) {
    throw new TypeError(`invalid Git commit: ${value}`);
  }
  return value as GitCommit;
}

function contractOperation(name: EndpointName): EmbeddedOperation {
  const operation = CONTRACT_FIXTURE.operations.find((candidate) => candidate.name === name);
  if (operation === undefined) {
    throw new Error(`generated contract omitted ${name}`);
  }
  return operation;
}

function stateFor(client: SymbolClient): ClientState {
  const state = CLIENT_STATES.get(client);
  if (state === undefined) {
    throw new TypeError("invalid SymbolClient instance");
  }
  return state;
}

function normalizedOrigin(origin: string | URL | undefined): URL {
  let value: URL;
  if (origin === undefined) {
    if (
      typeof globalThis.location !== "object" ||
      globalThis.location === null ||
      typeof globalThis.location.origin !== "string"
    ) {
      throw new TypeError("origin is required outside a same-origin browser context");
    }
    value = new URL(globalThis.location.origin);
  } else {
    value = new URL(origin.toString());
  }
  if (
    (value.protocol !== "http:" && value.protocol !== "https:") ||
    value.username !== "" ||
    value.password !== "" ||
    value.pathname !== "/" ||
    value.search !== "" ||
    value.hash !== ""
  ) {
    throw new TypeError(
      "origin must be an HTTP(S) origin without credentials, path, query, or fragment",
    );
  }
  return value;
}

function apiManualPath(manual: ApiManual): string {
  switch (manual) {
    case "index":
      return "/API/";
    case "javascript":
      return "/API/JS";
    case "typescript":
      return "/API/TS";
    case "python":
      return "/API/PY";
    case "shell":
      return "/API/SH";
    case "protocol":
      return "/API/CURL";
  }
}

function endpointUrl(client: SymbolClient, segments: readonly string[], trailing = false): URL {
  return new URL(encodedPath(segments, trailing), stateFor(client).origin);
}

function encodedPath(segments: readonly string[], trailing = false): string {
  const encoded = segments.map(encodeSegment).join("/");
  return `/${encoded}${trailing ? "/" : ""}`;
}

function encodeSegment(segment: string): string {
  if (segment === "" || segment === "." || segment === "..") {
    throw new TypeError("URL path segments must be non-empty and cannot be dot segments");
  }
  return encodeURIComponent(segment);
}

function validateSiteName(name: string): string {
  if (
    !/^[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?$/.test(name) ||
    name === "files" ||
    name === "stats" ||
    name === "symbol"
  ) {
    throw new TypeError("invalid Symbol site name");
  }
  return name;
}

function normalizedPath(path: string, allowEmpty = false): string {
  if (typeof path !== "string") {
    throw new TypeError("Symbol path must be a string");
  }
  const components = path
    .replace(/\\/g, "/")
    .split("/")
    .filter((part) => part !== "");
  if (
    components.some((part) => part === "." || part === ".." || /[\0-\x1f\x7f]/.test(part)) ||
    (!allowEmpty && components.length === 0)
  ) {
    throw new TypeError("invalid Symbol path");
  }
  return components.join("/");
}

function pathSegments(path: string): string[] {
  return path === "" ? [] : path.split("/");
}

function validateBasename(value: string): string {
  if (
    typeof value !== "string" ||
    value === "" ||
    value === "." ||
    value === ".." ||
    value.includes("/") ||
    value.includes("\\") ||
    /[\0-\x1f\x7f]/.test(value) ||
    ["FILES", "HASH", "UNDO", "EXPIRES", "symbol.toml", ".symbol-token", ".symbol-claim"].includes(
      value,
    )
  ) {
    throw new TypeError("allocation callback must return one safe basename");
  }
  return value;
}

function validatedNameFragment(value: string, label: string): string {
  if (
    typeof value !== "string" ||
    Array.from(value).some((character) => {
      const code = character.charCodeAt(0);
      return (
        code < 0x21 ||
        code > 0x7e ||
        character === "/" ||
        character === "\\" ||
        character === "?" ||
        character === "#"
      );
    })
  ) {
    throw new TypeError(`file ${label} must be a safe basename fragment`);
  }
  return value;
}

function validateHash(hash: string): Blake3 {
  const normalized = hash.startsWith("blake3:") ? hash.slice(7) : hash;
  if (!/^[0-9a-f]{64}$/.test(normalized)) {
    throw new TypeError("expected a lowercase Blake3 hash");
  }
  return normalized as Blake3;
}

function isSourceHash(hash: string): boolean {
  return /^[0-9a-f]{64}$/.test(hash) || /^blake3:[0-9a-f]{64}$/.test(hash);
}

function normalizedToken(value: string, label: string): string {
  if (typeof value !== "string" || !MEDIA_TOKEN.test(value)) {
    throw new TypeError(`invalid ${label}`);
  }
  return value.toLowerCase();
}

function normalizedParameterValue(name: string, value: string): string {
  if (
    typeof value !== "string" ||
    Array.from(value).some((character) => !isQuotedCharacter(character))
  ) {
    throw new TypeError("invalid media parameter value");
  }
  return name === "charset" ? value.toLowerCase() : value;
}

function isQuotedCharacter(character: string): boolean {
  if (character.length !== 1) {
    return false;
  }
  const code = character.charCodeAt(0);
  return (
    code === 0x09 ||
    code === 0x20 ||
    code === 0x21 ||
    (code >= 0x23 && code <= 0x5b) ||
    (code >= 0x5d && code <= 0x7e) ||
    (code >= 0x80 && code <= 0xff)
  );
}

function serializedParameterValue(value: string): string {
  if (MEDIA_TOKEN.test(value)) {
    return value;
  }
  return `"${value.replace(/([\\"])/g, "\\$1")}"`;
}

function validateRetryPolicy(policy: RetryPolicy): void {
  if (
    !Number.isSafeInteger(policy.maxAttempts) ||
    policy.maxAttempts < 1 ||
    !Number.isFinite(policy.initialDelayMs) ||
    policy.initialDelayMs < 0 ||
    !Number.isFinite(policy.maximumDelayMs) ||
    policy.maximumDelayMs < 0 ||
    !Number.isFinite(policy.backoffFactor) ||
    policy.backoffFactor < 1 ||
    (policy.jitter !== "none" && policy.jitter !== "full") ||
    !policy.retryStatuses.every(
      (status) => Number.isSafeInteger(status) && status >= 100 && status <= 599,
    )
  ) {
    throw new TypeError("invalid retry policy");
  }
}

class InternalNetworkRequestError extends Error {
  readonly cause: unknown;

  constructor(cause: unknown) {
    super(cause instanceof Error ? cause.message : "network request failed");
    this.name = "NetworkRequestError";
    this.cause = cause;
  }
}

function retryableError(error: unknown, policy: RetryPolicy): boolean {
  if (error instanceof InternalNetworkRequestError) {
    return policy.retryNetworkErrors;
  }
  if (error instanceof SymbolApiError) {
    if (
      error instanceof ApiCompatibilityError ||
      error instanceof MalformedResponseError ||
      error instanceof UnexpectedResponseError
    ) {
      return false;
    }
    return policy.retryStatuses.includes(error.status);
  }
  return false;
}

function retryDelay(error: unknown, policy: RetryPolicy, retryIndex: number): number {
  const exponential = Math.min(
    policy.maximumDelayMs,
    policy.initialDelayMs * policy.backoffFactor ** retryIndex,
  );
  let delay = policy.jitter === "full" ? Math.random() * exponential : exponential;
  if (policy.honorRetryAfter && error instanceof SymbolApiError) {
    const retryAfter = parsedRetryAfter(error.response.headers.get("Retry-After"));
    if (retryAfter !== null) {
      delay = Math.min(policy.maximumDelayMs, retryAfter);
    }
  }
  return Math.max(0, delay);
}

function parsedRetryAfter(value: string | null): number | null {
  if (value === null) {
    return null;
  }
  if (/^[0-9]+$/.test(value)) {
    return Number(value) * 1000;
  }
  if (
    !/^(?:Mon|Tue|Wed|Thu|Fri|Sat|Sun), [0-9]{2} (?:Jan|Feb|Mar|Apr|May|Jun|Jul|Aug|Sep|Oct|Nov|Dec) [0-9]{4} [0-9]{2}:[0-9]{2}:[0-9]{2} GMT$/.test(
      value,
    )
  ) {
    return null;
  }
  const timestamp = Date.parse(value);
  return Number.isNaN(timestamp) ? null : Math.max(0, timestamp - Date.now());
}

function immutableDate(value: Date): Date {
  const snapshot = new Date(value.getTime());
  const reject = (): never => {
    throw new TypeError("operation attempt dates are immutable");
  };
  for (const name of [
    "setDate",
    "setFullYear",
    "setHours",
    "setMilliseconds",
    "setMinutes",
    "setMonth",
    "setSeconds",
    "setTime",
    "setUTCDate",
    "setUTCFullYear",
    "setUTCHours",
    "setUTCMilliseconds",
    "setUTCMinutes",
    "setUTCMonth",
    "setUTCSeconds",
    "setYear",
  ]) {
    Object.defineProperty(snapshot, name, {
      configurable: false,
      enumerable: false,
      writable: false,
      value: reject,
    });
  }
  return Object.freeze(snapshot);
}

function abortableDelay(milliseconds: number, signal: AbortSignal): Promise<void> {
  if (milliseconds === 0) {
    return signal.aborted ? Promise.reject(signal.reason) : Promise.resolve();
  }
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => {
      signal.removeEventListener("abort", aborted);
      resolve();
    }, milliseconds);
    const aborted = (): void => {
      clearTimeout(timer);
      reject(signal.reason);
    };
    signal.addEventListener("abort", aborted, { once: true });
  });
}

function linkSignal(signal: AbortSignal | null, controller: AbortController): (() => void) | null {
  if (signal === null) {
    return null;
  }
  const abort = (): void => controller.abort(signal.reason);
  if (signal.aborted) {
    abort();
    return null;
  }
  signal.addEventListener("abort", abort, { once: true });
  return () => signal.removeEventListener("abort", abort);
}

function replayBody(body: BodyInit | null): ReplayBody {
  if (body === null) {
    return { replayable: true, next: () => null };
  }
  if (typeof body === "string") {
    return { replayable: true, next: () => body };
  }
  if (body instanceof Blob) {
    return { replayable: true, next: () => body };
  }
  if (body instanceof URLSearchParams) {
    const encoded = body.toString();
    return { replayable: true, next: () => new URLSearchParams(encoded) };
  }
  if (body instanceof ArrayBuffer) {
    const bytes = body.slice(0);
    return { replayable: true, next: () => bytes.slice(0) };
  }
  if (ArrayBuffer.isView(body)) {
    const bytes = new Uint8Array(body.buffer, body.byteOffset, body.byteLength).slice();
    return { replayable: true, next: () => bytes.slice() };
  }
  return { replayable: false, next: () => body };
}

function responseStatus(value: unknown): number | null {
  if (value instanceof Response) {
    return value.status;
  }
  if (isRecord(value) && typeof value.status === "number") {
    return value.status;
  }
  return null;
}

async function disposeOwnedValue(value: unknown): Promise<void> {
  if (
    (typeof value === "object" || typeof value === "function") &&
    value !== null &&
    Symbol.asyncDispose in value
  ) {
    const disposer = value[Symbol.asyncDispose];
    if (typeof disposer === "function") {
      await disposer.call(value);
    }
  }
}

function ownedResponse(response: Response): DisposableResponse {
  let disposePromise: Promise<void> | null = null;
  trackOwnedResponseBody(response);
  Object.defineProperty(response, Symbol.asyncDispose, {
    configurable: false,
    enumerable: false,
    writable: false,
    value: async (): Promise<void> => {
      if (disposePromise === null) {
        disposePromise = cancelOwnedResponseBody(response);
      }
      await disposePromise;
    },
  });
  return response as DisposableResponse;
}

type InternalOwnedReader = ReadableStreamDefaultReader<Uint8Array> | ReadableStreamBYOBReader;

const INTERNAL_OWNED_READERS = new WeakMap<ReadableStream<Uint8Array>, InternalOwnedReader>();

function trackOwnedResponseBody(response: Response): void {
  const body = response.body;
  if (body === null || INTERNAL_OWNED_READERS.has(body)) {
    return;
  }
  const getReader = body.getReader.bind(body);
  Object.defineProperty(body, "getReader", {
    configurable: false,
    enumerable: false,
    writable: false,
    value: ((options?: ReadableStreamGetReaderOptions): InternalOwnedReader => {
      const reader = options?.mode === "byob" ? getReader({ mode: "byob" }) : getReader();
      INTERNAL_OWNED_READERS.set(body, reader);
      return reader;
    }) as typeof body.getReader,
  });
}

async function cancelOwnedResponseBody(response: Response): Promise<void> {
  const body = response.body;
  if (body === null) {
    return;
  }
  const reader = INTERNAL_OWNED_READERS.get(body);
  if (reader !== undefined) {
    try {
      await reader.cancel();
    } finally {
      reader.releaseLock();
      INTERNAL_OWNED_READERS.delete(body);
    }
    return;
  }
  await body.cancel();
}

function effectiveIdempotencyKey(provided: string | undefined): string {
  const key = provided === undefined ? generatedIdempotencyKey() : provided;
  if (
    key.length < 1 ||
    key.length > 256 ||
    Array.from(key).some((character) => {
      const code = character.charCodeAt(0);
      return code < 0x21 || code > 0x7e;
    })
  ) {
    throw new TypeError("idempotency key must be 1-256 visible ASCII characters");
  }
  return key;
}

function generatedIdempotencyKey(): string {
  const crypto = globalThis.crypto;
  if (crypto === undefined) {
    throw new Error("secure random generation is unavailable");
  }
  if (typeof crypto.randomUUID === "function") {
    return crypto.randomUUID();
  }
  const bytes = crypto.getRandomValues(new Uint8Array(16));
  bytes[6] = (bytes[6] & 0x0f) | 0x40;
  bytes[8] = (bytes[8] & 0x3f) | 0x80;
  const hex = Array.from(bytes, (byte) => byte.toString(16).padStart(2, "0")).join("");
  return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`;
}

function phaseIdempotencyKey(key: string, phase: "propose" | "finalize" | "cancel"): string {
  const candidate = `${key}:${phase}`;
  if (candidate.length <= 256) {
    return candidate;
  }
  return `${key.slice(0, 220)}:${phase}:${stableKeyHash(key)}`;
}

function stableKeyHash(value: string): string {
  let hash = 0xcbf29ce484222325n;
  for (const character of value) {
    hash ^= BigInt(character.charCodeAt(0));
    hash = BigInt.asUintN(64, hash * 0x100000001b3n);
  }
  return hash.toString(16).padStart(16, "0");
}

function mergedHeaders(input: HeadersInit | undefined, protocol: Headers): Headers {
  const headers = new Headers(input);
  protocol.forEach((value, name) => {
    headers.set(name, value);
  });
  return headers;
}

function mergedOptions(base: RequestOptions, call: RequestOptions): RequestOptions {
  const headers = mergedHeaders(base.headers, new Headers(call.headers));
  return {
    token: call.token === undefined ? base.token : call.token,
    signal: call.signal === undefined ? base.signal : call.signal,
    headers,
  };
}

function mergedMutationOptions(base: RequestOptions, call: MutationOptions): MutationOptions {
  const merged = mergedOptions(base, call);
  return {
    ...merged,
    ifMatch: call.ifMatch,
    idempotencyKey: call.idempotencyKey,
  };
}

function addAuthorization(headers: Headers, state: ClientState, options: RequestOptions): void {
  const token = options.token === undefined ? state.token : options.token;
  if (token !== null && token !== undefined) {
    headers.set("Authorization", `Bearer ${token}`);
  }
}

function creationHeaders(creatorClaim: string | null): Headers {
  const headers = new Headers();
  if (creatorClaim !== null) {
    headers.set("Creator-Claim", creatorClaim);
  }
  return headers;
}

function publishHeaders(options: PublishOptions, creatorClaim: string | null): Headers {
  const headers = creationHeaders(creatorClaim);
  if (options.mediaType !== undefined) {
    headers.set("Content-Type", mediaTypeValue(options.mediaType).toString());
  }
  if (options.filename !== undefined) {
    const filename = validateBasename(options.filename).replace(/(["\\])/g, "\\$1");
    headers.set("Content-Disposition", `attachment; filename="${filename}"`);
  }
  if (options.unpack === true) {
    headers.set("Unpack", "true");
  }
  if (options.managed === true) {
    headers.set("Management-Action", "claim");
  }
  return headers;
}

function fileGetHeaders(options: FileGetOptions): Headers {
  const headers = new Headers();
  if (options.range !== undefined) {
    headers.set("Range", options.range);
  }
  if (options.ifRange !== undefined) {
    headers.set("If-Range", options.ifRange);
  }
  if (options.ifNoneMatch !== undefined) {
    headers.set("If-None-Match", options.ifNoneMatch);
  }
  return headers;
}

function mediaTypeValue(value: MediaType | string): MediaType {
  return typeof value === "string" ? MediaType.parse(value) : value;
}

function mediaTypeForBody(body: BodyInit): MediaType {
  if (body instanceof Blob && body.type !== "") {
    return MediaType.parse(body.type);
  }
  if (typeof body === "string") {
    return MediaTypes.Text;
  }
  return MediaTypes.Binary;
}

function formatForMediaType(value: string): ContentFormat {
  if (value === "") {
    return ContentFormats.Binary;
  }
  let mediaType: MediaType;
  try {
    mediaType = MediaType.parse(value);
  } catch {
    return ContentFormats.Binary;
  }
  const formats = Object.values(ContentFormats);
  return (
    formats.find((format) => format.mediaType.essence === mediaType.essence) ||
    new ContentFormat(mediaType)
  );
}

function allocationHeaders(
  mediaType: MediaType,
  naming: GeneratedNameParts,
  extension: string | null,
  expiry: Expiry | undefined,
): Headers {
  const headers = new Headers({
    "Allocation-Action": "create",
    "Content-Type": mediaType.toString(),
  });
  const prefix = naming.prefix === undefined ? "" : validatedNameFragment(naming.prefix, "prefix");
  const suffix = naming.suffix === undefined ? "" : validatedNameFragment(naming.suffix, "suffix");
  if (prefix !== "") {
    headers.set("File-Prefix", prefix);
  }
  if (suffix !== "") {
    headers.set("File-Suffix", suffix);
  }
  if (extension !== null) {
    headers.set("File-Extension", normalizeExtension(extension));
  }
  addExpiryHeaders(headers, expiry);
  return headers;
}

function allocationPath(site: string, folder: string): string {
  return encodedPath([site, ...pathSegments(folder)], true);
}

function addExpiryHeaders(headers: Headers, expiry: Expiry | undefined): void {
  if (expiry === undefined) {
    return;
  }
  headers.set("Expiry-Mode", expiry.mode);
  switch (expiry.mode) {
    case "never":
      return;
    case "relative":
      headers.set("Expiry-In", expiry.duration);
      return;
    case "absolute": {
      const at = expiry.at instanceof Date ? expiry.at.toISOString() : expiry.at;
      if (Number.isNaN(Date.parse(at))) {
        throw new TypeError("absolute expiry must be a valid timestamp");
      }
      headers.set("Expiry-At", at);
      return;
    }
    case "decay":
      headers.set("Expiry-Min-Age", expiry.minAge);
      headers.set("Expiry-Max-Age", expiry.maxAge);
      headers.set("Expiry-Max-Size", String(expiry.maxSize));
      headers.set("Expiry-Power", String(expiry.power));
      return;
  }
}

function archiveFormat(value: string): "tar" | "tar.gz" | "zip" {
  if (value === "tar" || value === "tar.gz" || value === "zip") {
    return value;
  }
  throw new TypeError("archive format must be tar, tar.gz, or zip");
}

interface PreparedSplices {
  readonly headers: Headers;
  readonly body: Blob;
}

interface InternalSnapshotSplice {
  readonly offset: number;
  readonly deleteBytes: number;
  readonly insert: Uint8Array<ArrayBuffer> | Blob;
  readonly insertBytes: number;
}

async function prepareSplices(
  changes: readonly InternalSnapshotSplice[],
  requested: "auto" | "headers" | "framed",
): Promise<PreparedSplices> {
  const descriptors = changes.map((change) => ({
    offset: change.offset,
    deleteBytes: change.deleteBytes,
    insertBytes: change.insertBytes,
  }));
  const encoded = descriptors
    .map(
      (change) =>
        `offset=${change.offset}; delete=${change.deleteBytes}; insert=${change.insertBytes}`,
    )
    .join(",");
  const canUseHeaders =
    descriptors.length <= CONTRACT_FIXTURE.splice.header_max_descriptors &&
    new TextEncoder().encode(encoded).byteLength <= CONTRACT_FIXTURE.splice.header_max_bytes;
  if (requested === "headers" && !canUseHeaders) {
    throw new RangeError("splice descriptors exceed the header format limits");
  }
  const useHeaders = requested === "headers" || (requested === "auto" && canUseHeaders);
  if (useHeaders) {
    return {
      headers: new Headers({ Splice: encoded }),
      body: new Blob(changes.map((change) => change.insert)),
    };
  }
  if (
    descriptors.length === 0 ||
    descriptors.length > CONTRACT_FIXTURE.splice.frame_max_descriptors ||
    descriptors.length * 24 > CONTRACT_FIXTURE.splice.frame_max_metadata_bytes
  ) {
    throw new RangeError("splice descriptors exceed the framed format limits");
  }
  const metadata = new Uint8Array(16 + descriptors.length * 24);
  metadata.set([0x53, 0x59, 0x4d, 0x53, 0x50, 0x4c, 0x31, 0x00], 0);
  const view = new DataView(metadata.buffer);
  view.setUint32(8, descriptors.length, false);
  view.setUint32(12, 0, false);
  let tableAt = 16;
  for (const descriptor of descriptors) {
    view.setBigUint64(tableAt, BigInt(descriptor.offset), false);
    view.setBigUint64(tableAt + 8, BigInt(descriptor.deleteBytes), false);
    view.setBigUint64(tableAt + 16, BigInt(descriptor.insertBytes), false);
    tableAt += 24;
  }
  return {
    headers: new Headers({ "Content-Type": CONTRACT_FIXTURE.splice.media_type }),
    body: new Blob([metadata, ...changes.map((change) => change.insert)]),
  };
}

function snapshotSplices(changes: Iterable<ByteSplice>): readonly InternalSnapshotSplice[] {
  const snapshots = Array.from(changes, (change) => {
    const insert = snapshotSpliceInsertion(change.insert);
    return Object.freeze({
      offset: change.offset,
      deleteBytes: change.deleteBytes,
      insert,
      insertBytes: insert instanceof Blob ? insert.size : insert.byteLength,
    });
  });
  validateSplices(snapshots);
  return Object.freeze(snapshots);
}

function validateSplices(changes: readonly InternalSnapshotSplice[]): void {
  if (changes.length === 0) {
    throw new RangeError("at least one splice is required");
  }
  let previousEnd = 0;
  let totalInsertBytes = 0;
  for (const [index, change] of changes.entries()) {
    if (
      !Number.isSafeInteger(change.offset) ||
      change.offset < 0 ||
      !Number.isSafeInteger(change.deleteBytes) ||
      change.deleteBytes < 0
    ) {
      throw new RangeError("splice offsets and delete lengths must be non-negative safe integers");
    }
    if (index > 0 && change.offset < previousEnd) {
      throw new RangeError("splice descriptors must be ordered and non-overlapping");
    }
    previousEnd = change.offset + change.deleteBytes;
    if (!Number.isSafeInteger(previousEnd)) {
      throw new RangeError("splice range exceeds JavaScript's safe integer range");
    }
    totalInsertBytes += change.insertBytes;
    if (!Number.isSafeInteger(totalInsertBytes)) {
      throw new RangeError("splice insertion total exceeds JavaScript's safe integer range");
    }
  }
}

function snapshotSpliceInsertion(
  value: ArrayBuffer | ArrayBufferView | Blob,
): Uint8Array<ArrayBuffer> | Blob {
  if (value instanceof Blob) {
    return value.slice(0, value.size, value.type);
  }
  if (value instanceof ArrayBuffer) {
    return new Uint8Array(value.slice(0));
  }
  if (ArrayBuffer.isView(value)) {
    return bodyBytes(value);
  }
  throw new TypeError("splice insertion must be an ArrayBuffer, view, or Blob");
}

function bodyBytes(value: ArrayBuffer | ArrayBufferView): Uint8Array<ArrayBuffer> {
  const source =
    value instanceof ArrayBuffer
      ? new Uint8Array(value)
      : new Uint8Array(value.buffer, value.byteOffset, value.byteLength);
  const copy = new Uint8Array(source.byteLength);
  copy.set(source);
  return copy;
}

function validateHostedResponse(response: Response): void {
  if (response.status === 304) {
    quotedHashHeader(response, "ETag");
    requireCacheControl(response);
    forbidHeaders(response, ["Content-Type", "Content-Length", "Content-Range", "Location"]);
    return;
  }
  if (response.status === 307) {
    requiredHeader(response, "Location");
    forbidHeaders(response, ["Content-Type", "Content-Length", "Content-Range", "ETag"]);
    return;
  }
  MediaType.parse(requiredHeader(response, "Content-Type"));
  quotedHashHeader(response, "ETag");
  requireCacheControl(response);
  if (requiredHeader(response, "Accept-Ranges") !== "bytes") {
    throw malformed(response, "Accept-Ranges must be bytes");
  }
  const contentLength = unsignedHeader(response, "Content-Length");
  if (response.status === 206) {
    const range = /^bytes ([0-9]+)-([0-9]+)\/([0-9]+)$/.exec(
      requiredHeader(response, "Content-Range"),
    );
    if (range === null) {
      throw malformed(response, "Content-Range must use canonical byte-range syntax");
    }
    const start = decimalUnsigned(range[1], "Content-Range start");
    const end = decimalUnsigned(range[2], "Content-Range end");
    const total = decimalUnsigned(range[3], "Content-Range total");
    if (start > end || end >= total || contentLength !== end - start + 1) {
      throw malformed(response, "Content-Range and Content-Length disagree");
    }
  } else {
    forbidHeaders(response, ["Content-Range", "Location"]);
  }
}

function quotedHashHeader(response: Response, name: string): EntityTag {
  const value = requiredHeader(response, name);
  const match = /^"([0-9a-f]{64})"$/.exec(value);
  if (match === null) {
    throw malformed(response, `${name} must be a quoted blake3 hash`);
  }
  return match[1] as EntityTag;
}

function quotedTreeHashHeader(response: Response, name: string): TreeHash {
  const value = requiredHeader(response, name);
  const match = /^"(blake3:[0-9a-f]{64})"$/.exec(value);
  if (match === null) {
    throw malformed(response, `${name} must be a quoted tree hash`);
  }
  return match[1] as TreeHash;
}

function requireCacheControl(response: Response): void {
  const value = requiredHeader(response, "Cache-Control");
  if (value !== "no-cache" && value !== "public, max-age=31536000, immutable") {
    throw malformed(response, "unexpected Cache-Control policy");
  }
}

function forbidHeaders(response: Response, names: readonly string[]): void {
  for (const name of names) {
    if (response.headers.has(name)) {
      throw malformed(response, `response unexpectedly included ${name}`);
    }
  }
}

async function decodeStats(response: Response): Promise<SymbolStats> {
  const value = await jsonValue(response);
  const root = exactRecord(
    value,
    [
      "sites",
      "files",
      "aliases",
      "blobs",
      "bytes",
      "logical_bytes",
      "saved_bytes",
      "saved_fraction",
      "file_sizes",
      "blob_sizes",
      "serving",
    ],
    "stats",
  );
  const serving = exactRecord(root.serving, ["cache", "readers"], "stats.serving");
  const cache = exactRecord(serving.cache, ["hits", "misses", "evictions"], "stats.serving.cache");
  const readers = exactRecord(
    serving.readers,
    ["operations", "waits", "wait_micros", "query_micros"],
    "stats.serving.readers",
  );
  return Object.freeze({
    sites: unsigned(root.sites, "stats.sites"),
    files: unsigned(root.files, "stats.files"),
    aliases: unsigned(root.aliases, "stats.aliases"),
    blobs: unsigned(root.blobs, "stats.blobs"),
    bytes: unsigned(root.bytes, "stats.bytes"),
    logicalBytes: unsigned(root.logical_bytes, "stats.logical_bytes"),
    savedBytes: unsigned(root.saved_bytes, "stats.saved_bytes"),
    savedFraction: finite(root.saved_fraction, "stats.saved_fraction"),
    fileSizes: decodeDistribution(root.file_sizes, "stats.file_sizes"),
    blobSizes: decodeDistribution(root.blob_sizes, "stats.blob_sizes"),
    serving: Object.freeze({
      cache: Object.freeze({
        hits: unsigned(cache.hits, "stats.serving.cache.hits"),
        misses: unsigned(cache.misses, "stats.serving.cache.misses"),
        evictions: unsigned(cache.evictions, "stats.serving.cache.evictions"),
      }),
      readers: Object.freeze({
        operations: unsigned(readers.operations, "stats.serving.readers.operations"),
        waits: unsigned(readers.waits, "stats.serving.readers.waits"),
        waitMicros: unsigned(readers.wait_micros, "stats.serving.readers.wait_micros"),
        queryMicros: unsigned(readers.query_micros, "stats.serving.readers.query_micros"),
      }),
    }),
  });
}

function decodeDistribution(value: unknown, label: string): SizeDistribution {
  const record = exactRecord(
    value,
    ["min", "p25", "median", "mean", "p75", "max", "iqr", "stddev"],
    label,
  );
  return Object.freeze({
    min: nullableNumber(record.min, `${label}.min`),
    p25: nullableNumber(record.p25, `${label}.p25`),
    median: nullableNumber(record.median, `${label}.median`),
    mean: nullableNumber(record.mean, `${label}.mean`),
    p75: nullableNumber(record.p75, `${label}.p75`),
    max: nullableNumber(record.max, `${label}.max`),
    iqr: nullableNumber(record.iqr, `${label}.iqr`),
    stddev: nullableNumber(record.stddev, `${label}.stddev`),
  });
}

async function decodeDirectoryListing(response: Response): Promise<DirectoryListing> {
  quotedHashHeader(response, "ETag");
  requireCacheControl(response);
  const value = exactRecord(
    await jsonValue(response),
    ["path", "files", "aliases", "bytes", "entries"],
    "directory listing",
  );
  const entries = array(value.entries, "directory listing.entries").map((entry, index) => {
    const record = recordValue(entry, `directory listing.entries[${index}]`);
    const kind = stringValue(record.kind, `directory listing.entries[${index}].kind`);
    if (kind === "builtin") {
      exactKeys(record, ["kind", "name", "files", "bytes"], "directory builtin entry");
      requireNull(record.files, "directory builtin entry.files");
      if (unsigned(record.bytes, "directory builtin entry.bytes") !== 0) {
        throw new TypeError("directory builtin entry.bytes must be zero");
      }
      return Object.freeze({
        kind,
        name: stringValue(record.name, "directory builtin entry.name"),
        files: null,
        bytes: 0 as const,
      });
    }
    if (kind === "site" || kind === "directory") {
      exactKeys(record, ["kind", "name", "files", "bytes"], `directory ${kind} entry`);
      return Object.freeze({
        kind,
        name: stringValue(record.name, "directory entry.name"),
        files: unsigned(record.files, "directory entry.files"),
        bytes: unsigned(record.bytes, "directory entry.bytes"),
      });
    }
    if (kind === "file") {
      exactKeys(record, ["kind", "name", "bytes"], "directory file entry");
      return Object.freeze({
        kind,
        name: stringValue(record.name, "directory entry.name"),
        bytes: unsigned(record.bytes, "directory entry.bytes"),
      });
    }
    if (kind === "alias") {
      exactKeys(
        record,
        ["kind", "name", "target", "target_kind", "dangling", "files", "bytes"],
        "directory alias entry",
      );
      return Object.freeze({
        kind,
        name: stringValue(record.name, "directory alias.name"),
        target: stringValue(record.target, "directory alias.target"),
        targetKind: nullableAliasKind(record.target_kind, "directory alias.target_kind"),
        dangling: booleanValue(record.dangling, "directory alias.dangling"),
        files: nullableUnsigned(record.files, "directory alias.files"),
        bytes: nullableUnsigned(record.bytes, "directory alias.bytes"),
      });
    }
    throw new TypeError(`unknown directory entry kind: ${kind}`);
  });
  return Object.freeze({
    path: stringValue(value.path, "directory listing.path"),
    files: unsigned(value.files, "directory listing.files"),
    aliases: unsigned(value.aliases, "directory listing.aliases"),
    bytes: unsigned(value.bytes, "directory listing.bytes"),
    entries: Object.freeze(entries),
  });
}

async function decodeCachedDirectoryListing(response: Response): Promise<CachedDirectoryListing> {
  const preserved = response.clone();
  if (response.status === 304) {
    const etag = quotedHashHeader(response, "ETag");
    requireCacheControl(response);
    forbidHeaders(response, ["Content-Type", "Content-Length", "Location"]);
    return Object.freeze({
      status: 304 as const,
      etag,
      cacheControl: requiredHeader(response, "Cache-Control"),
      response: preserved,
    });
  }
  if (response.status === 307) {
    const location = new URL(requiredHeader(response, "Location"), response.url);
    forbidHeaders(response, ["Content-Type", "Content-Length", "ETag"]);
    return Object.freeze({
      status: 307 as const,
      location,
      response: preserved,
    });
  }
  const listing = await decodeDirectoryListing(response);
  return Object.freeze({
    ...listing,
    status: 200 as const,
    etag: quotedHashHeader(preserved, "ETag"),
    cacheControl: requiredHeader(preserved, "Cache-Control"),
    response: preserved,
  });
}

async function decodeFileInventory(response: Response): Promise<FileInventory> {
  const value = exactRecord(
    await jsonValue(response),
    ["site", "content_revision", "tree_hash", "files", "aliases"],
    "file inventory",
  );
  const files = array(value.files, "file inventory.files").map((entry, index) => {
    const file = exactRecord(entry, ["path", "hash", "size"], `file inventory.files[${index}]`);
    return Object.freeze({
      path: stringValue(file.path, "inventory file.path"),
      hash: hashValue(file.hash, "inventory file.hash"),
      size: unsigned(file.size, "inventory file.size"),
    });
  });
  const aliases = array(value.aliases, "file inventory.aliases").map((entry, index) =>
    decodeAliasInventory(entry, `file inventory.aliases[${index}]`),
  );
  const contentRevision = unsigned(value.content_revision, "file inventory.content_revision");
  const treeHash = treeHashValue(value.tree_hash, "file inventory.tree_hash");
  if (
    unsignedHeader(response, "Content-Revision") !== contentRevision ||
    quotedTreeHashHeader(response, "ETag") !== treeHash
  ) {
    throw malformed(response, "inventory body disagrees with response headers");
  }
  requireCacheControl(response);
  return Object.freeze({
    site: stringValue(value.site, "file inventory.site"),
    contentRevision,
    treeHash,
    etag: requiredHeader(response, "ETag"),
    files: Object.freeze(files),
    aliases: Object.freeze(aliases),
  });
}

async function decodeCachedFileInventory(response: Response): Promise<CachedFileInventory> {
  const preserved = response.clone();
  if (response.status === 304) {
    const etag = quotedTreeHashHeader(response, "ETag");
    requireCacheControl(response);
    forbidHeaders(response, ["Content-Type", "Content-Length", "Content-Revision"]);
    return Object.freeze({
      status: 304 as const,
      etag,
      cacheControl: requiredHeader(response, "Cache-Control"),
      response: preserved,
    });
  }
  const inventory = await decodeFileInventory(response);
  return Object.freeze({
    ...inventory,
    status: 200 as const,
    response: preserved,
  });
}

function decodeAliasInventory(value: unknown, label: string): AliasInventoryEntry {
  const alias = exactRecord(
    value,
    ["path", "target", "target_kind", "dangling", "resolved_hash", "size"],
    label,
  );
  return Object.freeze({
    path: stringValue(alias.path, `${label}.path`),
    target: stringValue(alias.target, `${label}.target`),
    targetKind: nullableAliasKind(alias.target_kind, `${label}.target_kind`),
    dangling: booleanValue(alias.dangling, `${label}.dangling`),
    resolvedHash: nullableHash(alias.resolved_hash, `${label}.resolved_hash`),
    size: nullableUnsigned(alias.size, `${label}.size`),
  });
}

async function decodeUndoStack(response: Response): Promise<UndoStack> {
  const value = exactRecord(await jsonValue(response), ["site", "entries"], "undo stack");
  const kinds: readonly string[] = [
    "put",
    "delete_path",
    "delete_site",
    "copy",
    "move",
    "expiry",
    "expire_sweep",
    "put_file",
    "allocate",
    "replace",
    "splice",
    "alias",
  ];
  const entries = array(value.entries, "undo stack.entries").map((entry, index) => {
    const record = exactRecord(
      entry,
      ["token", "kind", "description", "created_at", "expires_at", "remaining_seconds"],
      `undo stack.entries[${index}]`,
    );
    const kind = stringValue(record.kind, "undo entry.kind");
    if (!kinds.includes(kind)) {
      throw new TypeError(`unknown undo kind: ${kind}`);
    }
    return Object.freeze({
      token: stringValue(record.token, "undo entry.token"),
      kind: kind as UndoKind,
      description: stringValue(record.description, "undo entry.description"),
      createdAt: dateValue(record.created_at, "undo entry.created_at"),
      expiresAt: dateValue(record.expires_at, "undo entry.expires_at"),
      remainingSeconds: unsigned(record.remaining_seconds, "undo entry.remaining_seconds"),
    });
  });
  return Object.freeze({
    site: stringValue(value.site, "undo stack.site"),
    entries: Object.freeze(entries),
  });
}

async function decodeExpirySiteReport(response: Response): Promise<ExpirySiteReport> {
  const value = exactRecord(await jsonValue(response), ["site", "entries"], "expiry inventory");
  const entries = array(value.entries, "expiry inventory.entries").map((entry, index) =>
    decodeExpiryValue(entry, `expiry inventory.entries[${index}]`),
  );
  return Object.freeze({
    site: stringValue(value.site, "expiry inventory.site"),
    entries: Object.freeze(entries),
  });
}

async function decodeExpiryReport(response: Response): Promise<ExpiryReport> {
  return decodeExpiryValue(await jsonValue(response), "expiry report");
}

function decodeExpiryValue(value: unknown, label: string): ExpiryReport {
  const report = exactRecord(
    value,
    [
      "target",
      "size",
      "refreshed_at",
      "own_policy",
      "inherited_caps",
      "effective_expires_at",
      "remaining_seconds",
      "limited_by",
    ],
    label,
  );
  const targetRecord = exactRecord(report.target, ["site", "path", "kind"], `${label}.target`);
  const targetKind = expiryKind(targetRecord.kind, `${label}.target.kind`);
  const targetPath = nullableString(targetRecord.path, `${label}.target.path`);
  const target: ExpiryTarget =
    targetKind === "site"
      ? Object.freeze({
          site: stringValue(targetRecord.site, `${label}.target.site`),
          kind: "site" as const,
          path: requireNull(targetPath, `${label}.target.path`),
        })
      : Object.freeze({
          site: stringValue(targetRecord.site, `${label}.target.site`),
          kind: targetKind,
          path: requiredNullableString(targetPath, `${label}.target.path`),
        });
  const caps = array(report.inherited_caps, `${label}.inherited_caps`).map((entry, index) => {
    const cap = exactRecord(
      entry,
      ["kind", "path", "expires_at"],
      `${label}.inherited_caps[${index}]`,
    );
    return Object.freeze({
      kind: expiryKind(cap.kind, "expiry cap.kind"),
      path: nullableString(cap.path, "expiry cap.path"),
      expiresAt: dateValue(cap.expires_at, "expiry cap.expires_at"),
    });
  });
  const limitedBy =
    report.limited_by === null
      ? null
      : (() => {
          const limit = exactRecord(report.limited_by, ["kind", "path"], `${label}.limited_by`);
          return Object.freeze({
            kind: expiryKind(limit.kind, "expiry limit.kind"),
            path: nullableString(limit.path, "expiry limit.path"),
          });
        })();
  return Object.freeze({
    target,
    size: unsigned(report.size, `${label}.size`),
    refreshedAt: nullableDate(report.refreshed_at, `${label}.refreshed_at`),
    ownPolicy:
      report.own_policy === null
        ? null
        : decodeExpiryPolicy(report.own_policy, `${label}.own_policy`),
    inheritedCaps: Object.freeze(caps),
    effectiveExpiresAt: nullableDate(report.effective_expires_at, `${label}.effective_expires_at`),
    remainingSeconds: nullableUnsigned(report.remaining_seconds, `${label}.remaining_seconds`),
    limitedBy,
  });
}

function decodeExpiryPolicy(value: unknown, label: string): ExpiryPolicy {
  const policy = exactRecord(
    value,
    [
      "mode",
      "min_age_seconds",
      "max_age_seconds",
      "max_size_bytes",
      "power",
      "retention_seconds",
      "expires_at",
    ],
    label,
  );
  const mode = stringValue(policy.mode, `${label}.mode`);
  const expiresAt = dateValue(policy.expires_at, `${label}.expires_at`);
  if (mode === "relative") {
    return Object.freeze({
      mode,
      minAgeSeconds: requireNull(policy.min_age_seconds, `${label}.min_age_seconds`),
      maxAgeSeconds: requireNull(policy.max_age_seconds, `${label}.max_age_seconds`),
      maxSizeBytes: requireNull(policy.max_size_bytes, `${label}.max_size_bytes`),
      power: requireNull(policy.power, `${label}.power`),
      retentionSeconds: unsigned(policy.retention_seconds, `${label}.retention_seconds`),
      expiresAt,
    });
  }
  if (mode === "absolute") {
    return Object.freeze({
      mode,
      minAgeSeconds: requireNull(policy.min_age_seconds, `${label}.min_age_seconds`),
      maxAgeSeconds: requireNull(policy.max_age_seconds, `${label}.max_age_seconds`),
      maxSizeBytes: requireNull(policy.max_size_bytes, `${label}.max_size_bytes`),
      power: requireNull(policy.power, `${label}.power`),
      retentionSeconds: requireNull(policy.retention_seconds, `${label}.retention_seconds`),
      expiresAt,
    });
  }
  if (mode === "decay") {
    return Object.freeze({
      mode,
      minAgeSeconds: unsigned(policy.min_age_seconds, `${label}.min_age_seconds`),
      maxAgeSeconds: unsigned(policy.max_age_seconds, `${label}.max_age_seconds`),
      maxSizeBytes: unsigned(policy.max_size_bytes, `${label}.max_size_bytes`),
      power: finite(policy.power, `${label}.power`),
      retentionSeconds: unsigned(policy.retention_seconds, `${label}.retention_seconds`),
      expiresAt,
    });
  }
  throw new TypeError(`unknown expiry mode: ${mode}`);
}

async function decodeManagementStatus(response: Response): Promise<ManagementStatus> {
  const value = exactRecord(await jsonValue(response), ["managed"], "management status");
  return Object.freeze({
    managed: booleanValue(value.managed, "management status.managed"),
  });
}

async function decodeSiteCreation(
  response: Response,
  idempotencyKey: string,
): Promise<SiteCreationReceipt> {
  const parsed = await decodePublishText(response, idempotencyKey);
  if (response.status !== 201 || !parsed.changed || parsed.undo === null) {
    throw malformed(response, "unnamed creation must be a changed 201 response");
  }
  return Object.freeze({
    ...parsed,
    status: 201 as const,
    changed: true as const,
    undo: parsed.undo,
  });
}

async function decodeSitePut(response: Response): Promise<SitePutReceipt> {
  const parsed = await decodePublishText(response, null);
  return Object.freeze({
    ...parsed,
    status: response.status as 200 | 201,
  }) as SitePutReceipt;
}

async function decodePublishText(
  response: Response,
  idempotencyKey: string | null,
): Promise<Omit<SitePutReceipt, "status">> {
  const preserved = response.clone();
  requireMediaType(response, "text/plain");
  const body = (await response.text()).trim();
  const match = /^ok ([^ ]+) ([^ ]+) \(([0-9]+) files, changed: (true|false)\)$/.exec(body);
  if (match === null) {
    throw malformed(response, "invalid site mutation receipt body");
  }
  const mutation = decodeMutationHeaders(response, idempotencyKey, match[4] === "true", preserved);
  return {
    ...mutation,
    site: match[1],
    files: unsigned(Number(match[3]), "site receipt.files"),
    creatorClaim: response.headers.get("Creator-Claim"),
    managementToken: response.headers.get("Management-Token"),
  } as Omit<SitePutReceipt, "status">;
}

async function decodeFilePut(response: Response): Promise<FilePutReceipt> {
  const preserved = response.clone();
  requireMediaType(response, "text/plain");
  const body = (await response.text()).trim();
  const match = /^ok \/.+ \(changed: (true|false)\)$/.exec(body);
  if (match === null) {
    throw malformed(response, "invalid file mutation receipt body");
  }
  return Object.freeze({
    ...decodeMutationHeaders(response, null, match[1] === "true", preserved),
    status: response.status as 200 | 201,
    creatorClaim: response.headers.get("Creator-Claim"),
    managementToken: response.headers.get("Management-Token"),
  }) as FilePutReceipt;
}

async function decodeCopy(response: Response, idempotencyKey: string | null): Promise<CopyReceipt> {
  const preserved = response.clone();
  requireMediaType(response, "text/plain");
  const body = (await response.text()).trim();
  const match = /^ok ([^ ]+) ([^ ]+) \(([0-9]+) files\)$/.exec(body);
  if (match === null) {
    throw malformed(response, "invalid copy receipt body");
  }
  const mutation = decodeMutationHeaders(response, idempotencyKey, true, preserved);
  if (mutation.undo === null) {
    throw malformed(response, "copy receipt omitted undo metadata");
  }
  return Object.freeze({
    ...mutation,
    status: 201 as const,
    changed: true as const,
    undo: mutation.undo,
    site: match[1],
    files: unsigned(Number(match[3]), "copy receipt.files"),
    creatorClaim: response.headers.get("Creator-Claim"),
    managementToken: response.headers.get("Management-Token"),
  });
}

async function decodeTextMutation(
  response: Response,
  idempotencyKey: string | null,
  changed: boolean,
): Promise<ChangedMutation | UnchangedMutation> {
  const preserved = response.clone();
  requireMediaType(response, "text/plain");
  await response.text();
  return decodeMutationHeaders(response, idempotencyKey, changed, preserved);
}

function decodeMutationHeaders(
  response: Response,
  idempotencyKey: string | null,
  changed: boolean,
  preserved: Response,
): ChangedMutation | UnchangedMutation {
  const undo = optionalUndoHeaders(response);
  if (changed !== (undo !== null)) {
    throw malformed(response, "mutation changed/undo metadata is inconsistent");
  }
  const replayHeader = response.headers.get("Idempotency-Replayed");
  if (replayHeader !== null && replayHeader !== "true") {
    throw malformed(response, "invalid Idempotency-Replayed header");
  }
  if (idempotencyKey === null && replayHeader !== null) {
    throw malformed(response, "non-idempotent mutation claimed to be replayed");
  }
  const base = {
    idempotencyKey,
    replayed: replayHeader === "true",
    location: new URL(requiredHeader(response, "Location")),
    etag: requiredHeader(response, "ETag"),
    contentRevision: unsignedHeader(response, "Content-Revision"),
    sanitizedManagementTokens: optionalUnsignedHeader(response, "Sanitized-Management-Tokens"),
    sanitizedCreatorClaims: optionalUnsignedHeader(response, "Sanitized-Creator-Claims"),
    response: preserved,
  };
  return changed
    ? Object.freeze({ ...base, changed: true as const, undo: undo as UndoReceipt })
    : Object.freeze({ ...base, changed: false as const, undo: null });
}

async function decodeDeleteReceipt(response: Response): Promise<DeleteReceipt> {
  const preserved = response.clone();
  requireMediaType(response, "text/plain");
  await response.text();
  const undo = requiredUndoHeaders(response);
  return Object.freeze({ status: 200 as const, undo, response: preserved });
}

async function decodeUndoMutation(response: Response): Promise<UndoMutationReceipt> {
  const preserved = response.clone();
  requireMediaType(response, "text/plain");
  const body = (await response.text()).trim();
  const match = /^restored .+ to (.+)$/.exec(body);
  if (match === null) {
    throw malformed(response, "invalid undo receipt body");
  }
  return Object.freeze({
    status: 200 as const,
    restoredAt: dateValue(match[1], "undo restored timestamp"),
    response: preserved,
  });
}

async function decodeArchive(
  response: Response,
  format: "tar" | "tar.gz" | "zip",
): Promise<ArchiveDownload> {
  return new ArchiveDownload(
    format,
    dispositionFilename(requiredHeader(response, "Content-Disposition")),
    unsignedHeader(response, "Content-Length"),
    response,
  );
}

async function decodeArchivePop(
  response: Response,
  format: "tar" | "tar.gz" | "zip",
): Promise<ArchivePopReceipt> {
  return new ArchivePopReceipt(
    format,
    dispositionFilename(requiredHeader(response, "Content-Disposition")),
    unsignedHeader(response, "Content-Length"),
    response,
    requiredUndoHeaders(response),
  );
}

async function decodeAliasReceipt(
  response: Response,
  idempotencyKey: string,
): Promise<AliasReceipt> {
  const preserved = response.clone();
  const value = recordValue(await jsonValue(response), "alias receipt");
  const mutation = decodeJsonMutation(value, response, idempotencyKey, preserved, [
    "path",
    "target",
    "target_kind",
    "dangling",
    "resolved_hash",
    "size",
  ]);
  return Object.freeze({
    ...mutation,
    status: response.status as 200 | 201,
    path: stringValue(value.path, "alias receipt.path"),
    target: stringValue(value.target, "alias receipt.target"),
    targetKind: nullableAliasKind(value.target_kind, "alias receipt.target_kind"),
    dangling: booleanValue(value.dangling, "alias receipt.dangling"),
    resolvedHash: nullableHash(value.resolved_hash, "alias receipt.resolved_hash"),
    size: nullableUnsigned(value.size, "alias receipt.size"),
  }) as AliasReceipt;
}

async function decodeAliasBatchReceipt(
  response: Response,
  idempotencyKey: string,
): Promise<AliasBatchReceipt> {
  const preserved = response.clone();
  const value = recordValue(await jsonValue(response), "alias batch receipt");
  const mutation = decodeJsonMutation(value, response, idempotencyKey, preserved, ["aliases"]);
  const aliases = array(value.aliases, "alias batch receipt.aliases").map((entry, index) =>
    decodeAliasInventory(entry, `alias batch receipt.aliases[${index}]`),
  );
  return Object.freeze({
    ...mutation,
    status: response.status as 200 | 201,
    aliases: Object.freeze(aliases),
  }) as AliasBatchReceipt;
}

async function decodeAllocationReceipt(
  response: Response,
  wireIdempotencyKey: string,
  exposedIdempotencyKey: string = wireIdempotencyKey,
  expectedProposal: ProposedFileName | null = null,
  expectedName: string | null = null,
): Promise<AllocationReceipt> {
  const preserved = response.clone();
  const value = recordValue(await jsonValue(response), "allocation receipt");
  const mutation = decodeJsonMutation(
    value,
    response,
    wireIdempotencyKey,
    preserved,
    ["outcome", "site", "path", "name", "url", "hash", "size", "blob_url", "naming"],
    exposedIdempotencyKey,
  );
  const outcome = enumValue(value.outcome, ["created", "existing"] as const, "allocation.outcome");
  const namingValue = recordValue(value.naming, "allocation.naming");
  const mode = enumValue(
    namingValue.mode,
    ["generated", "custom"] as const,
    "allocation.naming.mode",
  );
  const naming =
    mode === "custom"
      ? (() => {
          exactKeys(namingValue, ["mode"], "custom allocation naming");
          return Object.freeze({ mode: "custom" as const });
        })()
      : (() => {
          exactKeys(
            namingValue,
            ["mode", "prefix", "extension", "suffix"],
            "generated allocation naming",
          );
          return Object.freeze({
            mode: "generated" as const,
            prefix: stringValue(namingValue.prefix, "allocation.naming.prefix"),
            extension: stringValue(namingValue.extension, "allocation.naming.extension"),
            suffix: stringValue(namingValue.suffix, "allocation.naming.suffix"),
          });
        })();
  if (
    (outcome === "created") !== mutation.changed ||
    (outcome === "created" ? response.status !== 201 : response.status !== 200)
  ) {
    throw malformed(response, "allocation outcome/status/change discriminants disagree");
  }
  const site = validateSiteName(stringValue(value.site, "allocation.site"));
  const path = relativePathValue(value.path, "allocation.path");
  const name = validateBasename(stringValue(value.name, "allocation.name"));
  const url = urlValue(value.url, "allocation.url");
  const hash = hashValue(value.hash, "allocation.hash");
  const blobUrl = urlValue(value.blob_url, "allocation.blob_url");
  const pathParts = pathSegments(path);
  if (
    pathParts[pathParts.length - 1] !== name ||
    url.href !== mutation.location.href ||
    !url.pathname.endsWith(encodedPath([site, ...pathParts])) ||
    !blobUrl.pathname.endsWith(encodedPath([".blob", site, hash.slice("blake3:".length)])) ||
    (expectedName !== null && name !== expectedName) ||
    (expectedProposal !== null &&
      (hash !== expectedProposal.hash ||
        unsigned(value.size, "allocation.size") !== expectedProposal.size))
  ) {
    throw malformed(response, "allocation path/name/url/hash relationships disagree");
  }
  return Object.freeze({
    ...mutation,
    status: response.status as 200 | 201,
    outcome,
    site,
    path,
    name,
    url,
    hash,
    size: unsigned(value.size, "allocation.size"),
    blobUrl,
    naming,
  }) as AllocationReceipt;
}

async function decodeFileReplaceReceipt(
  response: Response,
  idempotencyKey: string,
): Promise<FileReplaceReceipt> {
  const preserved = response.clone();
  const value = recordValue(await jsonValue(response), "replace receipt");
  const mutation = decodeJsonMutation(value, response, idempotencyKey, preserved, [
    "outcome",
    "old_path",
    "new_path",
    "relocated",
    "old_hash",
    "new_hash",
    "size",
  ]);
  const outcome = enumValue(
    value.outcome,
    ["replaced", "relocated", "unchanged"] as const,
    "replace.outcome",
  );
  const relocated = booleanValue(value.relocated, "replace.relocated");
  if ((outcome === "unchanged") === mutation.changed || (outcome === "relocated") !== relocated) {
    throw malformed(response, "replace outcome discriminants disagree");
  }
  const oldPath = relativePathValue(value.old_path, "replace.old_path");
  const newPath = relativePathValue(value.new_path, "replace.new_path");
  const oldHash = hashValue(value.old_hash, "replace.old_hash");
  const newHash = hashValue(value.new_hash, "replace.new_hash");
  if (
    relocated !== (oldPath !== newPath) ||
    (outcome === "unchanged" && oldHash !== newHash) ||
    (outcome === "replaced" && oldPath !== newPath)
  ) {
    throw malformed(response, "replace path/hash relationships disagree");
  }
  return Object.freeze({
    ...mutation,
    status: 200 as const,
    outcome,
    oldPath,
    newPath,
    relocated,
    oldHash,
    newHash,
    size: unsigned(value.size, "replace.size"),
  }) as FileReplaceReceipt;
}

async function decodeSpliceReceipt(
  response: Response,
  idempotencyKey: string,
  expectedSplices: number,
  expectedSizeDelta: number,
): Promise<SpliceReceipt> {
  const preserved = response.clone();
  const value = recordValue(await jsonValue(response), "splice receipt");
  const mutation = decodeJsonMutation(value, response, idempotencyKey, preserved, [
    "old_path",
    "new_path",
    "relocated",
    "old_hash",
    "new_hash",
    "old_size",
    "new_size",
    "splices",
  ]);
  const oldPath = relativePathValue(value.old_path, "splice.old_path");
  const newPath = relativePathValue(value.new_path, "splice.new_path");
  const relocated = booleanValue(value.relocated, "splice.relocated");
  const oldHash = hashValue(value.old_hash, "splice.old_hash");
  const newHash = hashValue(value.new_hash, "splice.new_hash");
  const oldSize = unsigned(value.old_size, "splice.old_size");
  const newSize = unsigned(value.new_size, "splice.new_size");
  const splices = unsigned(value.splices, "splice.splices");
  if (
    relocated !== (oldPath !== newPath) ||
    splices !== expectedSplices ||
    newSize !== oldSize + expectedSizeDelta ||
    (!mutation.changed && (oldHash !== newHash || oldPath !== newPath))
  ) {
    throw malformed(response, "splice path/hash/size relationships disagree");
  }
  return Object.freeze({
    ...mutation,
    status: 200 as const,
    oldPath,
    newPath,
    relocated,
    oldHash,
    newHash,
    oldSize,
    newSize,
    splices,
  }) as SpliceReceipt;
}

function decodeJsonMutation(
  value: Record<string, unknown>,
  response: Response,
  idempotencyKey: string,
  preserved: Response,
  operationFields: readonly string[],
  exposedIdempotencyKey: string = idempotencyKey,
): ChangedMutation | UnchangedMutation {
  const mutationFields = [
    "changed",
    "replayed",
    "idempotency_key",
    "location",
    "etag",
    "content_revision",
    "sanitized_management_tokens",
    "sanitized_creator_claims",
    "undo",
  ];
  exactKeys(value, [...operationFields, ...mutationFields], "mutation receipt");
  const wireKey = stringValue(value.idempotency_key, "mutation.idempotency_key");
  if (wireKey !== idempotencyKey) {
    throw malformed(response, "mutation receipt idempotency key disagrees with request");
  }
  const changed = booleanValue(value.changed, "mutation.changed");
  const undo = value.undo === null ? null : decodeUndoValue(value.undo, "mutation.undo");
  if (changed !== (undo !== null)) {
    throw malformed(response, "mutation changed/undo fields disagree");
  }
  const replayed = booleanValue(value.replayed, "mutation.replayed");
  validateReplayHeader(response, replayed);
  const location = urlValue(value.location, "mutation.location");
  const etag = treeHashValue(value.etag, "mutation.etag");
  const contentRevision = unsigned(value.content_revision, "mutation.content_revision");
  if (
    requiredHeader(response, "Location") !== location.href ||
    requiredHeader(response, "ETag") !== `"${etag}"` ||
    unsignedHeader(response, "Content-Revision") !== contentRevision
  ) {
    throw malformed(response, "mutation receipt disagrees with response headers");
  }
  const headerUndo = optionalUndoHeaders(response);
  if (
    (undo === null) !== (headerUndo === null) ||
    (undo !== null &&
      headerUndo !== null &&
      (undo.token !== headerUndo.token ||
        undo.expiresAt.getTime() !== headerUndo.expiresAt.getTime()))
  ) {
    throw malformed(response, "mutation undo body disagrees with response headers");
  }
  const base = {
    idempotencyKey: exposedIdempotencyKey,
    replayed,
    location,
    etag,
    contentRevision,
    sanitizedManagementTokens: unsigned(
      value.sanitized_management_tokens,
      "mutation.sanitized_management_tokens",
    ),
    sanitizedCreatorClaims: unsigned(
      value.sanitized_creator_claims,
      "mutation.sanitized_creator_claims",
    ),
    response: preserved,
  };
  return changed
    ? Object.freeze({ ...base, changed: true as const, undo: undo as UndoReceipt })
    : Object.freeze({ ...base, changed: false as const, undo: null });
}

async function decodeProposalReceipt(
  response: Response,
  expectedIdempotencyKey: string,
): Promise<ProposalWireReceipt> {
  const value = exactRecord(
    await jsonValue(response),
    ["allocation_token", "expires_at", "proposal", "idempotency_key", "replayed"],
    "allocation proposal",
  );
  const proposal = exactRecord(
    value.proposal,
    ["folder", "default_name", "hash", "size", "media_type", "inferred_extension"],
    "allocation proposal.proposal",
  );
  const idempotencyKey = stringValue(value.idempotency_key, "proposal.idempotency_key");
  const replayed = booleanValue(value.replayed, "proposal.replayed");
  if (idempotencyKey !== expectedIdempotencyKey) {
    throw malformed(response, "proposal idempotency key disagrees with request");
  }
  validateReplayHeader(response, replayed);
  return Object.freeze({
    allocationToken: stringValue(value.allocation_token, "proposal.allocation_token"),
    expiresAt: dateValue(value.expires_at, "proposal.expires_at"),
    proposal: Object.freeze({
      folder: stringValue(proposal.folder, "proposal.folder"),
      defaultName: stringValue(proposal.default_name, "proposal.default_name"),
      hash: hashValue(proposal.hash, "proposal.hash"),
      size: unsigned(proposal.size, "proposal.size"),
      mediaType: stringValue(proposal.media_type, "proposal.media_type"),
      inferredExtension: nullableString(proposal.inferred_extension, "proposal.inferred_extension"),
    }),
    idempotencyKey,
    replayed,
  });
}

async function decodeCancellationReceipt(
  response: Response,
  key: string,
  allocationToken: string,
): Promise<void> {
  const value = exactRecord(
    await jsonValue(response),
    ["allocation_token", "cancelled", "idempotency_key", "replayed"],
    "allocation cancellation",
  );
  const replayed = booleanValue(value.replayed, "cancellation.replayed");
  if (
    !booleanValue(value.cancelled, "cancellation.cancelled") ||
    stringValue(value.allocation_token, "cancellation.allocation_token") !== allocationToken ||
    stringValue(value.idempotency_key, "cancellation.idempotency_key") !== key
  ) {
    throw malformed(response, "invalid allocation cancellation receipt");
  }
  validateReplayHeader(response, replayed);
}

function validateReplayHeader(response: Response, replayed: boolean): void {
  const header = response.headers.get("Idempotency-Replayed");
  if ((header === "true") !== replayed || (header !== null && header !== "true")) {
    throw malformed(response, "Idempotency-Replayed disagrees with receipt");
  }
}

function optionalUndoHeaders(response: Response): UndoReceipt | null {
  const token = response.headers.get("Undo-Token");
  const expires = response.headers.get("Undo-Expires");
  if (token === null && expires === null) {
    return null;
  }
  if (token === null || expires === null) {
    throw malformed(response, "partial undo metadata");
  }
  return Object.freeze({
    token,
    expiresAt: dateValue(expires, "Undo-Expires"),
  });
}

function requiredUndoHeaders(response: Response): UndoReceipt {
  const undo = optionalUndoHeaders(response);
  if (undo === null) {
    throw malformed(response, "response omitted required undo metadata");
  }
  return undo;
}

function decodeUndoValue(value: unknown, label: string): UndoReceipt {
  const undo = exactRecord(value, ["token", "expires_at"], label);
  return Object.freeze({
    token: stringValue(undo.token, `${label}.token`),
    expiresAt: dateValue(undo.expires_at, `${label}.expires_at`),
  });
}

function errorForStatus(
  operation: EndpointName | "raw request",
  status: number,
  body: string,
  idempotencyKey: string | null,
  response: Response,
): SymbolApiError {
  switch (status) {
    case 400:
      return new ValidationError(
        "ValidationError",
        operation,
        status,
        body,
        idempotencyKey,
        response,
      );
    case 401: {
      const challenge = response.headers.get("WWW-Authenticate");
      if (challenge === null) {
        return new MalformedResponseError(
          "MalformedResponseError",
          operation,
          status,
          "401 response omitted WWW-Authenticate",
          idempotencyKey,
          response,
        );
      }
      return new UnauthorizedError(operation, body, idempotencyKey, response, challenge);
    }
    case 403:
      return new ForbiddenError(
        "ForbiddenError",
        operation,
        status,
        body,
        idempotencyKey,
        response,
      );
    case 404:
      return new NotFoundError("NotFoundError", operation, status, body, idempotencyKey, response);
    case 405:
      return new MethodNotAllowedError(operation, body, idempotencyKey, response);
    case 409:
      return new ConflictError("ConflictError", operation, status, body, idempotencyKey, response);
    case 412: {
      const etag = response.headers.get("ETag");
      const revision = response.headers.get("Content-Revision");
      if (etag === null || revision === null || !/^[0-9]+$/.test(revision)) {
        return new MalformedResponseError(
          "MalformedResponseError",
          operation,
          status,
          "412 response omitted valid ETag/Content-Revision",
          idempotencyKey,
          response,
        );
      }
      return new PreconditionFailedError(
        operation,
        body,
        idempotencyKey,
        response,
        etag,
        Number(revision),
      );
    }
    case 413:
      return new PayloadTooLargeError(
        "PayloadTooLargeError",
        operation,
        status,
        body,
        idempotencyKey,
        response,
      );
    case 416:
      return new RangeNotSatisfiableError(operation, body, idempotencyKey, response);
    case 500:
      return new ServerError("ServerError", operation, status, body, idempotencyKey, response);
    default:
      return new UnexpectedResponseError(
        "UnexpectedResponseError",
        operation,
        status,
        body,
        idempotencyKey,
        response,
      );
  }
}

async function jsonValue(response: Response): Promise<unknown> {
  requireMediaType(response, "application/json");
  return JSON.parse(await response.text()) as unknown;
}

function requireMediaType(response: Response, expected: string): void {
  const contentType = requiredHeader(response, "Content-Type");
  if (MediaType.parse(contentType).essence !== expected) {
    throw malformed(response, `expected ${expected}, received ${contentType}`);
  }
}

function requiredHeader(response: Response, name: string): string {
  const value = response.headers.get(name);
  if (value === null) {
    throw malformed(response, `response omitted ${name}`);
  }
  return value;
}

function unsignedHeader(response: Response, name: string): number {
  return decimalUnsigned(requiredHeader(response, name), name);
}

function optionalUnsignedHeader(response: Response, name: string): number {
  const value = response.headers.get(name);
  return value === null ? 0 : decimalUnsigned(value, name);
}

function malformed(response: Response, message: string): TypeError {
  return new TypeError(`malformed HTTP ${response.status} response: ${message}`);
}

function dispositionFilename(value: string): string {
  const match = /(?:^|;)\s*filename="((?:[^"\\]|\\.)*)"/i.exec(value);
  if (match === null) {
    throw new TypeError("Content-Disposition omitted a quoted filename");
  }
  return match[1].replace(/\\(.)/g, "$1");
}

function exactRecord(
  value: unknown,
  keys: readonly string[],
  label: string,
): Record<string, unknown> {
  const record = recordValue(value, label);
  exactKeys(record, keys, label);
  return record;
}

function recordValue(value: unknown, label: string): Record<string, unknown> {
  if (!isRecord(value)) {
    throw new TypeError(`${label} must be an object`);
  }
  return value;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function exactKeys(value: Record<string, unknown>, keys: readonly string[], label: string): void {
  const actual = Object.keys(value).sort();
  const expected = Array.from(keys).sort();
  if (actual.length !== expected.length || actual.some((key, index) => key !== expected[index])) {
    throw new TypeError(`${label} has unexpected or missing fields`);
  }
}

function array(value: unknown, label: string): unknown[] {
  if (!Array.isArray(value)) {
    throw new TypeError(`${label} must be an array`);
  }
  return value;
}

function stringValue(value: unknown, label: string): string {
  if (typeof value !== "string") {
    throw new TypeError(`${label} must be a string`);
  }
  return value;
}

function hashValue(value: unknown, label: string): TreeHash {
  const hash = stringValue(value, label);
  if (!/^blake3:[0-9a-f]{64}$/.test(hash)) {
    throw new TypeError(`${label} must be a strict blake3 hash`);
  }
  return hash as TreeHash;
}

function treeHashValue(value: unknown, label: string): string {
  return hashValue(value, label);
}

function nullableHash(value: unknown, label: string): string | null {
  return value === null ? null : hashValue(value, label);
}

function relativePathValue(value: unknown, label: string): string {
  const path = stringValue(value, label);
  if (
    path.length === 0 ||
    path.startsWith("/") ||
    path.endsWith("/") ||
    path.includes("\\") ||
    path.split("/").some((segment) => segment === "" || segment === "." || segment === "..") ||
    Array.from(path).some((character) => {
      const code = character.charCodeAt(0);
      return code < 0x20 || code === 0x7f;
    })
  ) {
    throw new TypeError(`${label} must be a normalized relative file path`);
  }
  return path;
}

function decimalUnsigned(value: string, label: string): number {
  if (!/^(?:0|[1-9][0-9]*)$/.test(value)) {
    throw new TypeError(`${label} must use canonical unsigned decimal syntax`);
  }
  return unsigned(Number(value), label);
}

function nullableString(value: unknown, label: string): string | null {
  return value === null ? null : stringValue(value, label);
}

function requiredNullableString(value: string | null, label: string): string {
  if (value === null) {
    throw new TypeError(`${label} must be a string`);
  }
  return value;
}

function booleanValue(value: unknown, label: string): boolean {
  if (typeof value !== "boolean") {
    throw new TypeError(`${label} must be a boolean`);
  }
  return value;
}

function finite(value: unknown, label: string): number {
  if (typeof value !== "number" || !Number.isFinite(value)) {
    throw new TypeError(`${label} must be a finite number`);
  }
  return value;
}

function unsigned(value: unknown, label: string): number {
  if (typeof value !== "number" || !Number.isSafeInteger(value) || value < 0) {
    throw new TypeError(`${label} must be a non-negative safe integer`);
  }
  return value;
}

function nullableUnsigned(value: unknown, label: string): number | null {
  return value === null ? null : unsigned(value, label);
}

function nullableNumber(value: unknown, label: string): number | null {
  return value === null ? null : finite(value, label);
}

function dateValue(value: unknown, label: string): Date {
  const text = stringValue(value, label);
  if (
    !/^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-5][0-9](?:\.[0-9]+)?(?:Z|[+-][0-9]{2}:[0-9]{2})$/.test(
      text,
    )
  ) {
    throw new TypeError(`${label} must be an RFC 3339 timestamp`);
  }
  const milliseconds = Date.parse(text);
  if (Number.isNaN(milliseconds)) {
    throw new TypeError(`${label} must be an RFC 3339 timestamp`);
  }
  return immutableDate(new Date(milliseconds));
}

function nullableDate(value: unknown, label: string): Date | null {
  return value === null ? null : dateValue(value, label);
}

function urlValue(value: unknown, label: string): URL {
  try {
    return new URL(stringValue(value, label));
  } catch {
    throw new TypeError(`${label} must be an absolute URL`);
  }
}

function nullableAliasKind(value: unknown, label: string): "file" | "directory" | null {
  return value === null ? null : enumValue(value, ["file", "directory"] as const, label);
}

function expiryKind(value: unknown, label: string): "site" | "folder" | "file" {
  return enumValue(value, ["site", "folder", "file"] as const, label);
}

function enumValue<const T extends readonly string[]>(
  value: unknown,
  allowed: T,
  label: string,
): T[number] {
  const text = stringValue(value, label);
  if (!allowed.includes(text)) {
    throw new TypeError(`${label} has an unknown value`);
  }
  return text;
}

function requireNull(value: unknown, label: string): null {
  if (value !== null) {
    throw new TypeError(`${label} must be null`);
  }
  return null;
}
