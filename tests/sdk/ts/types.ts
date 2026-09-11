import {
  type AliasReceipt,
  type AllocationReceipt,
  type ApiVersion,
  type ArchiveDownload,
  BodyNotReplayableError,
  type CachedApiIdentity,
  type CachedFileInventory,
  type CachedTextAsset,
  ContentFormats,
  type ExpiryReport,
  type FileInventory,
  type FileReplaceReceipt,
  type ManagementClaimReceipt,
  MediaType,
  Operation,
  RetryPolicies,
  type SiteCreationReceipt,
  type SpliceReceipt,
  type SymbolApiVersion,
  SymbolClient,
  type SymbolStats,
} from "./symbol.js";

type Equal<Left, Right> =
  (<Value>() => Value extends Left ? 1 : 2) extends <Value>() => Value extends Right ? 1 : 2
    ? true
    : false;
type Expect<Value extends true> = Value;
type OptionalKeys<Value extends object> = {
  [Key in keyof Value]-?: object extends Pick<Value, Key> ? Key : never;
}[keyof Value];
type NoOptional<Value extends object> = Equal<OptionalKeys<Value>, never>;

const client = new SymbolClient({ origin: "http://127.0.0.1:4340" });
const version: ApiVersion = [1, 2, 3];
const site = client.site("hello");
const inventory: Operation<CachedFileInventory> = site.files();
const listing = site.files("assets");
const stats: Operation<SymbolStats> = client.stats();
const creation: Operation<SiteCreationReceipt> = client.create("hello");
const apiAsset: Operation<CachedTextAsset> = client.apiClient("symbol.ts");
const apiManual: Operation<CachedTextAsset> = client.apiManual("typescript");
const apiIdentity: Operation<CachedApiIdentity> = client.apiVersion();
const allocation: Operation<AllocationReceipt> = site.folder("generated").json({ value: 1 });
const alias: Operation<AliasReceipt> = site.alias("latest", "data.bin");
const replacement: Operation<FileReplaceReceipt> = site
  .file("data.bin")
  .replace(new Uint8Array([1]), { baseHash: "a".repeat(64) });
const splice: Operation<SpliceReceipt> = site
  .file("data.bin")
  .splice(
    { offset: 0, deleteBytes: 0, insert: new Uint8Array([1]) },
    { baseHash: "a".repeat(64), format: "headers" },
  );

type _InventoryExact = Expect<NoOptional<FileInventory>>;
type _StatsExact = Expect<NoOptional<SymbolStats>>;
type _AllocationExact = Expect<NoOptional<AllocationReceipt>>;
type _ReplacementExact = Expect<NoOptional<FileReplaceReceipt>>;
type _SpliceExact = Expect<NoOptional<SpliceReceipt>>;
type _ExpiryExact = Expect<NoOptional<ExpiryReport>>;
type _ApiVersionExact = Expect<NoOptional<SymbolApiVersion>>;

async function narrowing(): Promise<void> {
  const cached = await inventory;
  if (cached.status === 200) {
    cached.files.map((file) => file.path);
  } else {
    cached.etag.toUpperCase();
    // @ts-expect-error 304 responses do not lie about having inventory fields.
    cached.files;
  }
  const identity = await apiIdentity;
  if (identity.status === 200) {
    identity.identity.apiVersion.join(".");
    identity.identity.commit.length;
    identity.identity.dirty === true || identity.identity.dirty === false;
  } else {
    const absent: null = identity.identity;
    void absent;
  }
  const allocated = await allocation;
  if (allocated.changed) {
    allocated.undo.expiresAt.toISOString();
  } else {
    const absent: null = allocated.undo;
    void absent;
  }
  if (allocated.naming.mode === "generated") {
    allocated.naming.extension.toUpperCase();
  } else {
    const custom: "custom" = allocated.naming.mode;
    void custom;
  }

  const claimed: ManagementClaimReceipt = await site.management().claim();
  if (claimed.replayed) {
    const unavailable: null = claimed.managementToken;
    void unavailable;
  } else {
    claimed.managementToken.toUpperCase();
  }

  const replaced = await replacement;
  if (replaced.outcome === "relocated") {
    const relocated: true = replaced.relocated;
    void relocated;
  }

  await using operation = site.archive();
  await using archive: ArchiveDownload = await operation;
  await archive.blob();

  await using response = await site.file("data.bin").get();
  await response.arrayBuffer();

  const hosted = await site.file("config.json").json<{ enabled: boolean }>();
  hosted.enabled.valueOf();
}

const media = MediaType.application("problem+json", [["profile", "example"]]);
media.withCharset().structuredSuffix?.toUpperCase();
ContentFormats.JavaScript.extensions.map((extension) => extension.toUpperCase());
RetryPolicies.Default.retryStatuses.includes(503);
new BodyNotReplayableError();

// @ts-expect-error files accepts only no path/options or a string path.
site.files(42);
// @ts-expect-error replacement requires a base content hash.
site.file("data.bin").replace("body", {});
// @ts-expect-error byte allocation does not accept text.
site.folder().bytes("text");
// @ts-expect-error allocation callback must return a basename string.
site.folder().create("body", { name: () => 42 });
site.file("data.bin").splice(
  // @ts-expect-error splice insertions are binary, not strings.
  { offset: 0, deleteBytes: 0, insert: "x" },
  { baseHash: "a".repeat(64) },
);
// @ts-expect-error response DTO fields are readonly.
(stats as unknown as SymbolStats).sites = 2;
// @ts-expect-error Operation construction is internal to the generated SDK.
new Operation<SymbolStats>({} as never);
// @ts-expect-error unsupported site PUTs cannot accept idempotency keys.
site.put("body", { idempotencyKey: "unsupported" });
// @ts-expect-error unsupported file PUTs cannot accept idempotency keys.
site.file("data.bin").put("body", { idempotencyKey: "unsupported" });
// @ts-expect-error generated assets are a closed set.
client.apiClient("api.rb");
// @ts-expect-error manuals are a closed set.
client.apiManual("ruby");

void inventory;
void version;
void listing;
void stats;
void creation;
void apiAsset;
void apiManual;
void alias;
void splice;
void narrowing;
