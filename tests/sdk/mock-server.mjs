import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { webcrypto } from "node:crypto";
import { mkdtemp, readFile, readdir, rm, stat, writeFile } from "node:fs/promises";
import { STATUS_CODES } from "node:http";
import { createServer } from "node:net";
import { join, resolve } from "node:path";
import { tmpdir } from "node:os";
import { pathToFileURL } from "node:url";

if (globalThis.crypto === undefined) {
    Object.defineProperty(globalThis, "crypto", {
        configurable: true,
        value: webcrypto,
    });
}

const AMBIENT_REQUEST_HEADERS = new Set([
    "accept",
    "accept-encoding",
    "accept-language",
    "connection",
    "content-length",
    "host",
    "sec-fetch-mode",
    "transfer-encoding",
    "user-agent",
]);

export class ScenarioStep {
    constructor({
        operation,
        path,
        headers = {},
        body = null,
        status = null,
        responseHeaders = {},
        responseBody = null,
        fault = null,
        delayMs = 0,
        chunks = null,
    }) {
        if (typeof operation !== "string" || typeof path !== "string") {
            throw new TypeError("scenario operation and path are required strings");
        }
        this.operation = operation;
        this.path = path;
        this.headers = Object.freeze({ ...headers });
        this.body = body;
        this.status = status;
        this.responseHeaders = Object.freeze({ ...responseHeaders });
        this.responseBody = responseBody;
        this.fault = fault;
        this.delayMs = delayMs;
        this.chunks = chunks === null ? null : Object.freeze(Array.from(chunks));
        Object.freeze(this);
    }
}

export class MockServer {
    #fixture;
    #identity;
    #server;
    #origin;
    #steps;
    #requests;
    #failure;
    #sockets;

    constructor(fixture, identity = null) {
        validateFixture(fixture);
        this.#fixture = fixture;
        this.#identity = Object.freeze(identity === null
            ? fixtureIdentity(fixture)
            : validateIdentity(identity));
        this.#server = null;
        this.#origin = null;
        this.#steps = [];
        this.#requests = [];
        this.#failure = null;
        this.#sockets = new Set();
    }

    get origin() {
        if (this.#origin === null) {
            throw new Error("mock server has not started");
        }
        return this.#origin;
    }

    get fixture() {
        return this.#fixture;
    }

    get requests() {
        return Object.freeze(Array.from(this.#requests));
    }

    expect(...steps) {
        for (const value of steps) {
            this.#steps.push(value instanceof ScenarioStep ? value : new ScenarioStep(value));
        }
        return this;
    }

    async start() {
        if (this.#server !== null) {
            throw new Error("mock server already started");
        }
        this.#server = createServer((socket) => {
            this.#sockets.add(socket);
            socket.once("close", () => this.#sockets.delete(socket));
            socket.on("error", (error) => {
                if (!["ECONNRESET", "ERR_STREAM_WRITE_AFTER_END"].includes(error.code)) {
                    this.#failure = error;
                }
            });
            readRawRequest(socket).then((request) => {
                const response = new RawResponse(socket);
                return this.#handle(request, response).catch((error) => {
                    this.#failure = error;
                    if (!response.headersSent) {
                        response.writeHead(500, { "Content-Type": "text/plain" });
                    }
                    response.end(
                        `mock failure: ${error instanceof Error ? error.message : String(error)}\n`,
                    );
                });
            }).catch((error) => {
                this.#failure = error;
                if (!socket.destroyed && !socket.writableEnded) {
                    socket.end(
                        "HTTP/1.1 400 Bad Request\r\nConnection: close\r\nContent-Length: 0\r\n\r\n",
                    );
                }
            });
        });
        await new Promise((resolveStart, rejectStart) => {
            this.#server.once("error", rejectStart);
            this.#server.listen(0, "127.0.0.1", () => {
                this.#server.off("error", rejectStart);
                resolveStart();
            });
        });
        const address = this.#server.address();
        assert(address !== null && typeof address === "object");
        this.#origin = `http://127.0.0.1:${address.port}`;
        return this;
    }

    async verify() {
        if (this.#failure !== null) {
            throw this.#failure;
        }
        assert.equal(this.#steps.length, 0, `${this.#steps.length} mock scenario step(s) were not used`);
    }

    async stop({ verify = true } = {}) {
        if (this.#server === null) {
            return;
        }
        await new Promise((resolveStop, rejectStop) => {
            this.#server.close((error) => error === undefined ? resolveStop() : rejectStop(error));
            for (const socket of this.#sockets) {
                socket.destroy();
            }
        });
        this.#server = null;
        if (verify) {
            await this.verify();
        }
    }

    async #handle(request, response) {
        const url = new URL(request.url ?? "/", this.origin);
        const step = this.#steps.shift();
        assert(step !== undefined, `unexpected ${request.method} ${url.pathname}`);
        const operation = operationNamed(this.#fixture, step.operation);
        assert.equal(request.method, operation.method, `${step.operation} method`);
        assert.equal(url.pathname + url.search, step.path, `${step.operation} path`);
        validateIncomingHeaders(operation, request.headers, step.headers);
        const body = await collectBody(request);
        validateIncomingBody(step.body, body, step.operation);
        this.#requests.push(Object.freeze({
            operation: step.operation,
            method: request.method,
            path: url.pathname + url.search,
            headers: Object.freeze({ ...request.headers }),
            body: Buffer.from(body),
        }));
        if (step.fault === "drop") {
            request.socket.destroy();
            return;
        }
        if (step.fault !== null) {
            throw new Error(`unknown mock fault: ${step.fault}`);
        }
        if (step.delayMs > 0) {
            await new Promise((resolveDelay) => setTimeout(resolveDelay, step.delayMs));
        }
        const selectedStatus = step.status ?? operation.success_outcomes[0].status;
        const requestVariant = operation.name === "allocated file"
            ? request.headers["allocation-action"] ?? "create"
            : "default";
        const outcome = [
            ...operation.success_outcomes,
            ...operation.error_outcomes,
        ].find((candidate) =>
            candidate.status === selectedStatus
            && (candidate.request_variant === "default"
                || candidate.request_variant === requestVariant)
        );
        assert(outcome !== undefined, `${step.operation} status ${selectedStatus} is not in the fixture`);
        const canonical = canonicalOutcome(
            this.origin,
            operation,
            selectedStatus,
            request.headers,
            body,
            this.#requests,
            url.pathname,
        );
        const responseBody = normalizeBody(
            step.responseBody === null ? canonical.body : step.responseBody,
        );
        const headers = this.#identityHeaders({
            ...canonical.headers,
            ...step.responseHeaders,
        });
        if (
            outcome.body !== "empty"
            && !("Content-Length" in headers)
            && !("content-length" in headers)
        ) {
            headers["Content-Length"] = responseBody.byteLength.toString();
        }
        validateMockOutcome(operation, outcome, selectedStatus, headers, responseBody);
        response.writeHead(selectedStatus, headers);
        if (step.chunks === null) {
            response.end(responseBody);
            return;
        }
        for (const chunk of step.chunks) {
            response.write(normalizeBody(chunk));
            await new Promise((resolveChunk) => setTimeout(resolveChunk, 1));
        }
        response.end();
    }

    #identityHeaders(headers) {
        return {
            ...headers,
            "Symbol-API-Version": this.#identity.apiVersion,
            "Symbol-API-Revision": this.#identity.absoluteRevision.toString(),
            "Symbol-API-Source-Hash": this.#identity.sourceHash,
        };
    }
}

export async function startMockServer(fixture, identity = null) {
    return new MockServer(fixture, identity).start();
}

export async function startTemporarySymbol({ binary, dataRoot, root = process.cwd() }) {
    const child = spawn(binary, [
        "--bind", "127.0.0.1:0",
        "--root", dataRoot,
    ], {
        cwd: root,
        env: {
            ...process.env,
            RUST_LOG: "info",
            SYMBOL_PUBLIC_URL: "http://127.0.0.1:0",
        },
        stdio: ["ignore", "pipe", "pipe"],
    });
    let log = "";
    const origin = await new Promise((resolveOrigin, rejectOrigin) => {
        let settled = false;
        const observe = (chunk) => {
            log += chunk.toString();
            const match = /listening on 127\.0\.0\.1:([0-9]+)/.exec(log);
            if (!settled && match !== null && match[1] !== "0") {
                settled = true;
                resolveOrigin(`http://127.0.0.1:${match[1]}`);
            }
        };
        child.stdout.on("data", observe);
        child.stderr.on("data", observe);
        child.once("error", (error) => {
            if (!settled) {
                settled = true;
                rejectOrigin(error);
            }
        });
        child.once("exit", (code) => {
            if (!settled) {
                settled = true;
                rejectOrigin(new Error(`temporary Symbol exited ${code}\n${log}`));
            }
        });
    });
    await waitForTemporarySymbol(origin, child, () => log);
    return Object.freeze({
        origin,
        process: child,
        get log() {
            return log;
        },
        async stop() {
            child.kill("SIGTERM");
            await new Promise((resolveExit) => {
                if (child.exitCode !== null) {
                    resolveExit();
                } else {
                    child.once("exit", resolveExit);
                }
            });
        },
    });
}

async function waitForTemporarySymbol(origin, child, log) {
    for (let attempt = 0; attempt < 100; attempt += 1) {
        if (child.exitCode !== null) {
            throw new Error(`temporary Symbol exited ${child.exitCode}\n${log()}`);
        }
        try {
            const response = await fetch(`${origin}/STATS`);
            if (response.ok) {
                await response.arrayBuffer();
                return;
            }
        } catch {
            // The server announced its owned port before accepting this request.
        }
        await new Promise((resolveDelay) => setTimeout(resolveDelay, 20));
    }
    throw new Error(`temporary Symbol did not become ready\n${log()}`);
}

export async function locateGeneratedArtifacts(root = process.cwd()) {
    if (process.env.SYMBOL_GENERATED_DIR !== undefined) {
        const out = resolve(process.env.SYMBOL_GENERATED_DIR);
        const fixture = JSON.parse(await readFile(resolve(out, "symbol-contract.json"), "utf8"));
        validateFixture(fixture);
        return Object.freeze({
            directory: out,
            fixture,
            apiTs: resolve(out, "symbol.ts"),
            apiJs: resolve(out, "symbol.js"),
            apiGlobalJs: resolve(out, "symbol.global.js"),
            apiDeclarations: resolve(out, "symbol.d.ts"),
        });
    }
    const buildRoot = resolve(root, "target", "debug", "build");
    const ledger = await readLedger(resolve(root, "api-version.toml"));
    const candidates = [];
    for (const name of await readdir(buildRoot)) {
        if (!name.startsWith("symbol-")) {
            continue;
        }
        const out = resolve(buildRoot, name, "out");
        try {
            const fixture = JSON.parse(await readFile(resolve(out, "symbol-contract.json"), "utf8"));
            if (
                fixture.api_version === ledger.version
                && fixture.absolute_revision === ledger.absoluteRevision
                && fixture.source_hash === ledger.sourceHash
            ) {
                candidates.push({
                    out,
                    fixture,
                    modified: (await stat(resolve(out, "symbol.js"))).mtimeMs,
                });
            }
        } catch {
            // Unrelated or incomplete Cargo build directories are not candidates.
        }
    }
    candidates.sort((left, right) => right.modified - left.modified);
    assert(candidates.length > 0, "matching generated SDK artifacts were not found; run ./api-version update");
    const selected = candidates[0];
    return Object.freeze({
        directory: selected.out,
        fixture: selected.fixture,
        apiTs: resolve(selected.out, "symbol.ts"),
        apiJs: resolve(selected.out, "symbol.js"),
        apiGlobalJs: resolve(selected.out, "symbol.global.js"),
        apiDeclarations: resolve(selected.out, "symbol.d.ts"),
    });
}

export async function importGeneratedEsm(path) {
    const source = await readFile(path, "utf8");
    const directory = await mkdtemp(join(tmpdir(), "symbol-sdk-esm-"));
    const modulePath = join(directory, "api.mjs");
    await writeFile(modulePath, source);
    try {
        return await import(pathToFileURL(modulePath).href);
    } finally {
        await rm(directory, { recursive: true, force: true });
    }
}

export async function loadGeneratedUmd(path) {
    const { runInNewContext } = await import("node:vm");
    const source = await readFile(path, "utf8");
    const context = {
        AbortController,
        ArrayBuffer,
        Blob,
        Date,
        DataView,
        Error,
        Headers,
        Math,
        Number,
        Object,
        Promise,
        RangeError,
        Response,
        Set,
        Symbol,
        TextDecoder,
        TextEncoder,
        TypeError,
        URL,
        URLSearchParams,
        Uint8Array,
        WeakMap,
        clearTimeout,
        console,
        crypto: globalThis.crypto,
        fetch,
        setTimeout,
    };
    context.globalThis = context;
    runInNewContext(source, context, { filename: path });
    assert(context.SymbolAPI !== undefined, "UMD did not initialize SymbolAPI");
    assert.equal(context.Symbol, Symbol, "UMD overwrote the built-in Symbol");
    return Object.freeze({ exports: context.SymbolAPI, context });
}

class RawRequest {
    constructor(method, url, headers, body, socket) {
        this.method = method;
        this.url = url;
        this.headers = headers;
        this.body = body;
        this.socket = socket;
        Object.freeze(this.headers);
    }

    async *[Symbol.asyncIterator]() {
        if (this.body.byteLength > 0) {
            yield this.body;
        }
    }
}

class RawResponse {
    #socket;
    #status;
    #headers;
    #flushed;
    #chunked;
    #ended;

    constructor(socket) {
        this.#socket = socket;
        this.#status = 200;
        this.#headers = {};
        this.#flushed = false;
        this.#chunked = false;
        this.#ended = false;
    }

    get headersSent() {
        return this.#flushed || Object.keys(this.#headers).length > 0;
    }

    writeHead(status, headers = {}) {
        assert.equal(this.#flushed, false, "response headers were already sent");
        this.#status = status;
        this.#headers = { ...headers };
        return this;
    }

    write(value) {
        assert.equal(this.#ended, false, "response already ended");
        const bytes = normalizeBody(value);
        this.#flush();
        if (this.#chunked) {
            this.#socket.write(`${bytes.byteLength.toString(16)}\r\n`);
            this.#socket.write(bytes);
            this.#socket.write("\r\n");
        } else {
            this.#socket.write(bytes);
        }
        return true;
    }

    end(value = null) {
        if (this.#ended) {
            return;
        }
        this.#ended = true;
        if (value !== null && !this.#flushed) {
            const bytes = normalizeBody(value);
            if (!hasHeader(this.#headers, "content-length")) {
                this.#headers["Content-Length"] = bytes.byteLength.toString();
            }
            this.#flush();
            this.#socket.end(bytes);
            return;
        }
        if (value !== null) {
            this.write(value);
        } else {
            this.#flush();
        }
        if (this.#chunked) {
            this.#socket.end("0\r\n\r\n");
        } else {
            this.#socket.end();
        }
    }

    #flush() {
        if (this.#flushed) {
            return;
        }
        this.#flushed = true;
        this.#headers.Connection = "close";
        if (!hasHeader(this.#headers, "content-length")) {
            this.#headers["Transfer-Encoding"] = "chunked";
            this.#chunked = true;
        }
        const reason = STATUS_CODES[this.#status] ?? "Unknown";
        let encoded = `HTTP/1.1 ${this.#status} ${reason}\r\n`;
        for (const [name, value] of Object.entries(this.#headers)) {
            encoded += `${name}: ${String(value)}\r\n`;
        }
        encoded += "\r\n";
        this.#socket.write(encoded);
    }
}

function readRawRequest(socket) {
    return new Promise((resolveRequest, rejectRequest) => {
        let buffered = Buffer.alloc(0);
        const failed = (error) => {
            cleanup();
            rejectRequest(error);
        };
        const cleanup = () => {
            socket.off("data", received);
            socket.off("error", failed);
            socket.off("end", ended);
        };
        const ended = () => failed(new Error("connection ended before the HTTP request completed"));
        const received = (chunk) => {
            buffered = Buffer.concat([buffered, Buffer.from(chunk)]);
            try {
                const parsed = parseRawRequest(buffered, socket);
                if (parsed !== null) {
                    cleanup();
                    resolveRequest(parsed);
                }
            } catch (error) {
                failed(error);
            }
        };
        socket.on("data", received);
        socket.once("error", failed);
        socket.once("end", ended);
    });
}

function parseRawRequest(buffer, socket) {
    const headerEnd = buffer.indexOf("\r\n\r\n");
    if (headerEnd < 0) {
        return null;
    }
    const lines = buffer.subarray(0, headerEnd).toString("latin1").split("\r\n");
    const requestLine = /^([^ ]+) ([^ ]+) HTTP\/1\.[01]$/.exec(lines.shift() ?? "");
    if (requestLine === null) {
        throw new Error("malformed HTTP request line");
    }
    const headers = {};
    for (const line of lines) {
        const separator = line.indexOf(":");
        if (separator <= 0) {
            throw new Error("malformed HTTP request header");
        }
        const name = line.slice(0, separator).trim().toLowerCase();
        const value = line.slice(separator + 1).trim();
        headers[name] = headers[name] === undefined ? value : `${headers[name]}, ${value}`;
    }
    const payload = buffer.subarray(headerEnd + 4);
    let body;
    if ((headers["transfer-encoding"] ?? "").toLowerCase() === "chunked") {
        body = parseChunkedBody(payload);
        if (body === null) {
            return null;
        }
    } else {
        const lengthText = headers["content-length"] ?? "0";
        if (!/^[0-9]+$/.test(lengthText)) {
            throw new Error("invalid Content-Length");
        }
        const length = Number(lengthText);
        if (!Number.isSafeInteger(length) || payload.byteLength < length) {
            return null;
        }
        body = payload.subarray(0, length);
    }
    return new RawRequest(requestLine[1], requestLine[2], headers, body, socket);
}

function parseChunkedBody(payload) {
    let at = 0;
    const chunks = [];
    for (;;) {
        const lineEnd = payload.indexOf("\r\n", at);
        if (lineEnd < 0) {
            return null;
        }
        const sizeText = payload.subarray(at, lineEnd).toString("ascii").split(";", 1)[0];
        if (!/^[0-9a-f]+$/i.test(sizeText)) {
            throw new Error("invalid chunk size");
        }
        const size = Number.parseInt(sizeText, 16);
        at = lineEnd + 2;
        if (size === 0) {
            return payload.byteLength >= at + 2 ? Buffer.concat(chunks) : null;
        }
        if (payload.byteLength < at + size + 2) {
            return null;
        }
        chunks.push(payload.subarray(at, at + size));
        at += size;
        if (payload.toString("ascii", at, at + 2) !== "\r\n") {
            throw new Error("malformed chunk terminator");
        }
        at += 2;
    }
}

function hasHeader(headers, wanted) {
    return Object.keys(headers).some((name) => name.toLowerCase() === wanted);
}

function validateFixture(fixture) {
    assert.equal(typeof fixture, "object");
    assert.equal(fixture.fixture_version, 2);
    assert.match(fixture.api_version, /^\d+\.\d+\.\d+$/);
    assert(Number.isSafeInteger(fixture.absolute_revision) && fixture.absolute_revision > 0);
    assert.match(fixture.source_hash, /^[0-9a-f]{64}$/);
    assert.match(fixture.build.commit, /^(?:unknown|[0-9a-fA-F]{7,64})$/);
    assert.equal(typeof fixture.build.dirty, "boolean");
    assert.equal(fixture.operations.length, 40);
    assert.equal(new Set(fixture.operations.map((operation) => operation.name)).size, 40);
    for (const operation of fixture.operations) {
        assert.equal(operation.outcomes_exact, true, `${operation.name} outcomes must be exact`);
        for (const outcome of [...operation.success_outcomes, ...operation.error_outcomes]) {
            assert.equal(typeof outcome.body, "string");
            assert.equal(typeof outcome.schema, "string");
            assert.equal(typeof outcome.request_variant, "string");
            assert(Array.isArray(outcome.optional_headers));
            assert(Array.isArray(outcome.forbidden_headers));
        }
    }
}

function fixtureIdentity(fixture) {
    return Object.freeze({
        apiVersion: fixture.api_version,
        absoluteRevision: fixture.absolute_revision,
        sourceHash: fixture.source_hash,
    });
}

function validateIdentity(identity) {
    assert.match(identity.apiVersion, /^\d+\.\d+\.\d+$/);
    assert(Number.isSafeInteger(identity.absoluteRevision) && identity.absoluteRevision > 0);
    assert.match(identity.sourceHash, /^[0-9a-f]{64}$/);
    return Object.freeze({ ...identity });
}

function operationNamed(fixture, name) {
    const operation = fixture.operations.find((candidate) => candidate.name === name);
    assert(operation !== undefined, `fixture operation not found: ${name}`);
    return operation;
}

function validateIncomingHeaders(operation, actual, expected) {
    const declared = new Set(operation.request_headers.map((name) => name.toLowerCase()));
    for (const [name, value] of Object.entries(expected)) {
        const observed = actual[name.toLowerCase()];
        if (value instanceof RegExp) {
            assert.match(String(observed), value, `${operation.name} ${name}`);
        } else {
            assert.equal(observed, value, `${operation.name} ${name}`);
        }
    }
    for (const name of Object.keys(actual)) {
        if (!declared.has(name) && !AMBIENT_REQUEST_HEADERS.has(name)) {
            throw new Error(`${operation.name} sent undeclared request header ${name}`);
        }
    }
}

function validateIncomingBody(expected, actual, operation) {
    if (expected === null) {
        assert.equal(actual.byteLength, 0, `${operation} body must be empty`);
        return;
    }
    if (typeof expected === "function") {
        expected(Buffer.from(actual));
        return;
    }
    const normalized = normalizeBody(expected);
    assert.deepEqual(Buffer.from(actual), normalized, `${operation} body`);
}

function validateMockOutcome(operation, outcome, status, headers, body) {
    const normalized = new Map(
        Object.entries(headers).map(([name, value]) => [name.toLowerCase(), String(value)]),
    );
    for (const required of outcome.required_headers) {
        assert(normalized.has(required.toLowerCase()), `mock ${status} omitted ${required}`);
    }
    for (const forbidden of outcome.forbidden_headers) {
        assert(!normalized.has(forbidden.toLowerCase()), `mock ${status} included forbidden ${forbidden}`);
    }
    const declared = new Set([
        ...operation.response_headers.map((name) => name.toLowerCase()),
        ...outcome.required_headers.map((name) => name.toLowerCase()),
        ...outcome.optional_headers.map((name) => name.toLowerCase()),
    ]);
    const ambient = new Set([
        "content-length",
        "symbol-api-version",
        "symbol-api-revision",
        "symbol-api-source-hash",
    ]);
    for (const name of normalized.keys()) {
        if (!declared.has(name) && !ambient.has(name)) {
            throw new Error(`${operation.name} emitted undeclared response header ${name}`);
        }
    }
    const contentType = normalized.get("content-type") ?? "";
    switch (outcome.body) {
        case "empty":
            assert.equal(body.byteLength, 0);
            break;
        case "json":
            assert.match(contentType, /^application\/json(?:;|$)/i);
            validateSchema(outcome.schema, JSON.parse(body.toString("utf8")));
            break;
        case "plain_text":
            assert.match(contentType, /^text\//i);
            new TextDecoder("utf-8", { fatal: true }).decode(body);
            break;
        case "binary":
            break;
        default:
            throw new Error(`unknown fixture body kind: ${outcome.body}`);
    }
}

function validateSchema(schema, value) {
    const object = exactObject(value, schema);
    switch (schema) {
        case "api_version":
            exactKeys(object, ["api_version", "absolute_revision", "source_hash", "commit", "dirty"], schema);
            assert.match(object.api_version, /^\d+\.\d+\.\d+$/);
            assert(Number.isSafeInteger(object.absolute_revision));
            assert.match(object.source_hash, /^[0-9a-f]{64}$/);
            assert.match(object.commit, /^(?:unknown|[0-9a-fA-F]{7,64})$/);
            assert.equal(typeof object.dirty, "boolean");
            return;
        case "stats":
            exactKeys(object, [
                "sites", "files", "aliases", "blobs", "bytes", "logical_bytes", "saved_bytes",
                "saved_fraction", "file_sizes", "blob_sizes", "serving",
            ], schema);
            return;
        case "directory_listing":
            exactKeys(object, ["path", "files", "aliases", "bytes", "entries"], schema);
            assert(Array.isArray(object.entries));
            return;
        case "file_inventory":
            exactKeys(object, [
                "site", "created_at", "updated_at", "content_revision", "tree_hash", "events",
                "files", "aliases",
            ], schema);
            assert.match(object.tree_hash, /^blake3:[0-9a-f]{64}$/);
            assert(Array.isArray(object.events));
            object.events.forEach((event) => {
                exactKeys(event, ["kind", "at", "files"], schema);
                assert(["created", "publish", "rename", "restore"].includes(event.kind));
            });
            return;
        case "undo_stack":
            exactKeys(object, ["site", "entries"], schema);
            assert(Array.isArray(object.entries));
            return;
        case "management_status":
            exactKeys(object, ["managed"], schema);
            assert.equal(typeof object.managed, "boolean");
            return;
        case "expiry_report":
            validateExpiryReport(object, schema);
            return;
        case "expiry_site_report":
            exactKeys(object, ["site", "entries"], schema);
            assert(Array.isArray(object.entries));
            object.entries.forEach((entry) => validateExpiryReport(exactObject(entry, schema), schema));
            return;
        case "alias_receipt":
            exactKeys(object, [
                "path", "target", "target_kind", "dangling", "resolved_hash", "size",
                ...MUTATION_FIELDS,
            ], schema);
            return;
        case "alias_batch_receipt":
            exactKeys(object, ["aliases", ...MUTATION_FIELDS], schema);
            assert(Array.isArray(object.aliases));
            return;
        case "allocation_receipt":
            exactKeys(object, [
                "outcome", "site", "path", "name", "url", "hash", "size", "blob_url", "naming",
                ...MUTATION_FIELDS,
            ], schema);
            assert.match(object.hash, /^blake3:[0-9a-f]{64}$/);
            assert.equal(object.name, object.path.split("/").at(-1));
            return;
        case "allocation_proposal_receipt":
            exactKeys(object, [
                "allocation_token", "expires_at", "proposal", "idempotency_key", "replayed",
            ], schema);
            exactKeys(exactObject(object.proposal, "proposal"), [
                "folder", "default_name", "hash", "size", "media_type", "inferred_extension",
            ], "proposal");
            assert.match(object.proposal.hash, /^blake3:[0-9a-f]{64}$/);
            return;
        case "allocation_cancellation_receipt":
            exactKeys(object, [
                "allocation_token", "cancelled", "idempotency_key", "replayed",
            ], schema);
            assert.equal(object.cancelled, true);
            return;
        case "file_replace_receipt":
            exactKeys(object, [
                "outcome", "old_path", "new_path", "relocated", "old_hash", "new_hash", "size",
                ...MUTATION_FIELDS,
            ], schema);
            return;
        case "splice_receipt":
            exactKeys(object, [
                "old_path", "new_path", "relocated", "old_hash", "new_hash",
                "old_size", "new_size", "splices", ...MUTATION_FIELDS,
            ], schema);
            return;
        default:
            throw new Error(`JSON body used non-JSON schema ${schema}`);
    }
}

const MUTATION_FIELDS = [
    "changed", "replayed", "idempotency_key", "location", "etag", "content_revision",
    "sanitized_management_tokens", "sanitized_creator_claims", "undo",
];

function validateExpiryReport(object, label) {
    exactKeys(object, [
        "target", "size", "refreshed_at", "own_policy", "inherited_caps",
        "effective_expires_at", "remaining_seconds", "limited_by",
    ], label);
    const target = exactObject(object.target, `${label}.target`);
    exactKeys(target, ["site", "path", "kind"], `${label}.target`);
    assert(["site", "folder", "file"].includes(target.kind));
    if (target.kind === "site") {
        assert.equal(target.path, null);
    } else {
        assert.equal(typeof target.path, "string");
        assert(target.path.length > 0);
    }
}

function exactObject(value, label) {
    assert(value !== null && typeof value === "object" && !Array.isArray(value), `${label} object`);
    return value;
}

function exactKeys(object, expected, label) {
    assert.deepEqual(Object.keys(object).sort(), Array.from(expected).sort(), `${label} keys`);
}

function canonicalOutcome(
    origin,
    operation,
    status,
    requestHeaders,
    requestBody,
    requests,
    requestPath,
) {
    if (!operation.success_outcomes.some((outcome) => outcome.status === status)) {
        if (status === 416 && operation.name !== "file splice") {
            return {
                headers: {
                    "Content-Range": "bytes */8",
                    "Accept-Ranges": "bytes",
                    ETag: quotedRawHash("b"),
                    "Cache-Control": "no-cache",
                },
                body: Buffer.alloc(0),
            };
        }
        const headers = { "Content-Type": "text/plain; charset=utf-8" };
        if (status === 401) {
            headers["WWW-Authenticate"] = "Bearer realm=\"symbol\"";
            headers["Cache-Control"] = "no-store";
        }
        if (status === 412) {
            headers.ETag = `"blake3:${"b".repeat(64)}"`;
            headers["Content-Revision"] = "2";
        }
        return { headers, body: `error: mock ${operation.name}\n` };
    }
    const base = successHeaders(origin, operation.name);
    switch (operation.name) {
        case "docs":
            return textAsset(base, "<h1>Symbol</h1>\n", status);
        case "installer":
            return textAsset(base, "#!/bin/sh\n", status);
        case "client":
            return textAsset(base, "#!/bin/sh\n# symbol\n", status);
        case "api client asset":
            return textAsset(base, "export const generated = true;\n", status);
        case "api documentation":
            return textAsset(base, "# Symbol API\n", status);
        case "docs hash":
        case "installer hash":
        case "client hash":
        case "api client hash":
        case "file hash":
            return plain(base, `${"a".repeat(64)}\n`);
        case "api version":
            if (status === 304) {
                return empty({ ETag: quotedRawHash("a"), "Cache-Control": "no-cache" });
            }
            return json({
                ETag: quotedRawHash("a"),
                "Cache-Control": "no-cache",
            }, {
                api_version: "0.0.0",
                absolute_revision: 1,
                source_hash: "a".repeat(64),
                commit: "0123456789abcdef",
                dirty: false,
            });
        case "stats":
            return json(base, statsFixture());
        case "site listing":
        case "files subtree":
            if (status === 304) {
                return empty({ ETag: quotedRawHash("1"), "Cache-Control": "no-cache" });
            }
            if (status === 307) {
                return empty({
                    Location: `${origin}/hello/FILES/path/`,
                    "Content-Length": "0",
                });
            }
            return json({
                ETag: quotedRawHash("1"),
                "Cache-Control": "no-cache",
            }, listingFixture(operation.name === "site listing"));
        case "site redirect":
            return empty({ ...base, Location: "/hello/", "Content-Length": "0" });
        case "site index":
        case "site file":
        case "immutable blob":
            return hosted(base, status);
        case "unnamed put":
            return plainMutation(base, "ok made", true, 201);
        case "site put":
            return plainMutation(base, "ok hello", true, status);
        case "file put":
            return plainMutation(base, "ok /hello/path", true, status);
        case "site copy":
            return plainMutation(base, "ok copy", true, 201, "(2 files)");
        case "site move":
            return plainMutation(base, "moved old -> new", true, 200);
        case "site undo":
            return plain(base, "restored hello to 2026-08-26T12:00:00Z\n");
        case "file delete":
            return plain({
                ...base,
                "Undo-Token": "undo-delete",
                "Undo-Expires": "2026-08-26T16:00:00Z",
            }, "deleted hello/path\n");
        case "site pop":
        case "archive pop":
            return archive({
                ...base,
                "Undo-Token": "undo-pop",
                "Undo-Expires": "2026-08-26T16:00:00Z",
            }, "hello.tar.gz", false);
        case "archive get":
            return archive(base, "hello.tar.gz", true);
        case "files inventory":
            if (status === 304) {
                return empty({
                    ETag: quotedTreeHash("2"),
                    "Cache-Control": "no-cache",
                });
            }
            return json({
                ETag: quotedTreeHash("2"),
                "Content-Revision": "1",
                "Cache-Control": "no-cache",
            }, inventoryFixture());
        case "undo stack":
            return json({ ...base, "Cache-Control": "no-cache" }, undoFixture());
        case "expiry inventory":
            return json({ ...base, "Cache-Control": "no-cache" }, {
                site: "hello",
                entries: [expiryFixture()],
            });
        case "expiry target":
            return json(
                { ...base, "Cache-Control": "no-cache" },
                expiryFixture("file", decodeURIComponent(requestPath.split("/").at(-2))),
            );
        case "site expire":
        case "file expire":
            return json({
                Expires: "Wed, 26 Aug 2026 16:00:00 GMT",
                "Expiry-Mode": "relative",
                "Undo-Token": "undo-expiry",
                "Undo-Expires": "2026-08-26T16:00:00Z",
            }, operation.name === "site expire"
                ? expiryFixture("site", null)
                : expiryFixture("file", decodeURIComponent(requestPath.split("/").at(-1))));
        case "site management":
            return managementOutcome(base, requestHeaders);
        case "alias file":
            return jsonMutation(base, requestHeaders, {
                path: "latest",
                target: requestHeaders["alias-target"] ?? "assets/app.js",
                target_kind: "file",
                dangling: false,
                resolved_hash: `blake3:${"c".repeat(64)}`,
                size: 8,
            }, status);
        case "alias batch":
            return jsonMutation(base, requestHeaders, {
                aliases: [{
                    path: "latest",
                    target: "assets/app.js",
                    target_kind: "file",
                    dangling: false,
                    resolved_hash: `blake3:${"c".repeat(64)}`,
                    size: 8,
                }],
            }, status);
        case "allocated file":
            return allocationOutcome(base, requestHeaders, requestBody, status, requests);
        case "file replace":
            return jsonMutation(base, requestHeaders, {
                outcome: "replaced",
                old_path: "data.bin",
                new_path: "data.bin",
                relocated: false,
                old_hash: `blake3:${"d".repeat(64)}`,
                new_hash: `blake3:${"e".repeat(64)}`,
                size: 9,
            }, 200);
        case "file splice":
            {
                const descriptors = spliceDescriptors(requestHeaders, requestBody);
                const delta = descriptors.reduce(
                    (sum, descriptor) => sum + descriptor.insert - descriptor.delete,
                    0,
                );
                const oldSize = Math.max(
                    8,
                    ...descriptors.map((descriptor) => descriptor.offset + descriptor.delete),
                );
            return jsonMutation(base, requestHeaders, {
                old_path: "data.bin",
                new_path: "data.bin",
                relocated: false,
                old_hash: `blake3:${"d".repeat(64)}`,
                new_hash: `blake3:${"e".repeat(64)}`,
                    old_size: oldSize,
                    new_size: oldSize + delta,
                    splices: descriptors.length,
            }, 200);
            }
        default:
            throw new Error(`no canonical mock outcome for ${operation.name}`);
    }
}

function spliceDescriptors(headers, body) {
    if (headers.splice !== undefined) {
        return headers.splice.split(",").map((descriptor) => {
            const values = Object.fromEntries(
                descriptor.split(";").map((part) => part.trim().split("=")),
            );
            return {
                offset: Number(values.offset),
                delete: Number(values.delete),
                insert: Number(values.insert),
            };
        });
    }
    const count = body.readUInt32BE(8);
    const descriptors = [];
    for (let index = 0; index < count; index += 1) {
        const offset = 16 + index * 24;
        descriptors.push({
            offset: Number(body.readBigUInt64BE(offset)),
            delete: Number(body.readBigUInt64BE(offset + 8)),
            insert: Number(body.readBigUInt64BE(offset + 16)),
        });
    }
    return descriptors;
}

function successHeaders(origin, operation) {
    if ([
        "unnamed put", "site put", "site copy", "site move", "file put",
        "alias file", "alias batch", "allocated file", "file replace", "file splice",
    ].includes(operation)) {
        return {
            Location: `${origin}/hello/`,
            ETag: quotedTreeHash("2"),
            "Content-Revision": "1",
        };
    }
    return {};
}

function textAsset(base, body, status) {
    if (status === 304) {
        return empty({
            ETag: quotedRawHash("a"),
            "Cache-Control": "no-cache",
        });
    }
    const headers = {
        ...base,
        "Content-Type": "text/plain; charset=utf-8",
        ETag: quotedRawHash("a"),
        "Cache-Control": "no-cache",
    };
    return { headers, body };
}

function plain(base, body) {
    return { headers: { ...base, "Content-Type": "text/plain; charset=utf-8" }, body };
}

function json(base, body) {
    return { headers: { ...base, "Content-Type": "application/json" }, body: JSON.stringify(body) };
}

function empty(headers) {
    return { headers, body: Buffer.alloc(0) };
}

function quotedRawHash(character) {
    return `"${character.repeat(64)}"`;
}

function treeHash(character = "2") {
    return `blake3:${character.repeat(64)}`;
}

function quotedTreeHash(character = "2") {
    return `"${treeHash(character)}"`;
}

function hosted(base, status) {
    if (status === 304) {
        return empty({ ETag: quotedRawHash("b"), "Cache-Control": "no-cache" });
    }
    if (status === 307) {
        return empty({ Location: "/hello/folder/", "Content-Length": "0" });
    }
    const headers = {
        ...base,
        "Content-Type": "application/octet-stream",
        ETag: quotedRawHash("b"),
        "Cache-Control": "no-cache",
        "Accept-Ranges": "bytes",
    };
    if (status === 206) {
        headers["Content-Range"] = "bytes 0-1/8";
        return { headers, body: Buffer.from("ab") };
    }
    return { headers, body: Buffer.from("abcdefgh") };
}

function plainMutation(base, prefix, changed, status, suffix = "(2 files, changed: true)") {
    const headers = {
        ...base,
        "Content-Type": "text/plain; charset=utf-8",
        Location: base.Location,
        ETag: quotedTreeHash("2"),
        "Content-Revision": "1",
    };
    if (changed) {
        headers["Undo-Token"] = "undo-mutation";
        headers["Undo-Expires"] = "2026-08-26T16:00:00Z";
    }
    let body;
    if (prefix.startsWith("ok /")) {
        body = `${prefix} (changed: ${changed})\n`;
    } else if (prefix === "ok copy") {
        body = `ok copy ${base.Location.replace(/hello\/$/, "copy/")} ${suffix}\n`;
    } else if (prefix.startsWith("moved")) {
        body = `${prefix}\n`;
    } else {
        body = `${prefix} ${base.Location} (${suffix.replace(/^\(|\)$/g, "")})\n`;
    }
    return { headers, body, status };
}

function archive(base, filename, cached) {
    const body = Buffer.from("mock archive");
    return {
        headers: {
            ...base,
            "Content-Type": "application/gzip",
            "Content-Disposition": `attachment; filename="${filename}"`,
            ...(cached ? { "Cache-Control": "no-cache" } : {}),
        },
        body,
    };
}

function jsonMutation(base, requestHeaders, fields, status) {
    const replayed = requestHeaders["x-mock-replayed"] === "true";
    const changed = true;
    return json({
        ...base,
        "Undo-Token": "undo-json",
        "Undo-Expires": "2026-08-26T16:00:00Z",
    }, {
        ...fields,
        changed,
        replayed,
        idempotency_key: requestHeaders["idempotency-key"],
        location: base.Location,
        etag: treeHash(),
        content_revision: 1,
        sanitized_management_tokens: 0,
        sanitized_creator_claims: 0,
        undo: {
            token: "undo-json",
            expires_at: "2026-08-26T16:00:00Z",
        },
    });
}

function managementOutcome(base, requestHeaders) {
    const action = requestHeaders["management-action"];
    if (action === "status") {
        return json({ ...base, "Cache-Control": "no-store" }, { managed: false });
    }
    if (action === "release") {
        return json({ ...base, "Cache-Control": "no-store" }, { managed: false });
    }
    return json({
        ...base,
        "Cache-Control": "no-store",
        "Management-Token": "sym_mgmt_mock",
    }, { managed: true });
}

function allocationOutcome(base, requestHeaders, requestBody, status, requests) {
    const action = requestHeaders["allocation-action"] ?? "create";
    if (action === "propose") {
        return json(base, {
            allocation_token: "allocation-token",
            expires_at: "2026-08-26T16:00:00Z",
            proposal: {
                folder: "generated",
                default_name: `${"f".repeat(64)}.txt`,
                hash: `blake3:${"f".repeat(64)}`,
                size: requestBody.byteLength,
                media_type: requestHeaders["content-type"] ?? "application/octet-stream",
                inferred_extension: requestHeaders["file-extension"] ?? null,
            },
            idempotency_key: requestHeaders["idempotency-key"],
            replayed: false,
        });
    }
    if (action === "cancel") {
        return json(base, {
            allocation_token: requestHeaders["allocation-token"],
            cancelled: true,
            idempotency_key: requestHeaders["idempotency-key"],
            replayed: false,
        });
    }
    const naming = action === "finalize"
        ? { mode: "custom" }
        : {
            mode: "generated",
            prefix: requestHeaders["file-prefix"] ?? "",
            extension: requestHeaders["file-extension"] ?? "",
            suffix: requestHeaders["file-suffix"] ?? "",
        };
    const created = status === 201;
    const hash = `blake3:${"f".repeat(64)}`;
    const proposalRequest = requests.findLast((request) =>
        request.operation === "allocated file"
        && request.headers["allocation-action"] === "propose"
    );
    const size = action === "finalize"
        ? proposalRequest?.body.byteLength ?? 0
        : requestBody.byteLength;
    const path = action === "finalize"
        ? `generated/${requestHeaders["file-name"]}`
        : `generated/${requestHeaders["file-prefix"] ?? ""}${"f".repeat(64)}${requestHeaders["file-suffix"] ?? ""}${requestHeaders["file-extension"] ? `.${requestHeaders["file-extension"]}` : ""}`;
    const blobUrl = `${new URL(base.Location).origin}/.blob/hello/${"f".repeat(64)}`;
    const location = `${base.Location}${path}`;
    const headers = {
        ...base,
        Location: location,
        "Content-Location": blobUrl,
    };
    if (created) {
        headers["Undo-Token"] = "undo-allocation";
        headers["Undo-Expires"] = "2026-08-26T16:00:00Z";
    }
    return json(headers, {
        outcome: created ? "created" : "existing",
        site: "hello",
        path,
        name: path.split("/").at(-1),
        url: location,
        hash,
        size,
        blob_url: blobUrl,
        naming,
        changed: created,
        replayed: false,
        idempotency_key: requestHeaders["idempotency-key"],
        location,
        etag: treeHash(),
        content_revision: 1,
        sanitized_management_tokens: 0,
        sanitized_creator_claims: 0,
        undo: created
            ? { token: "undo-allocation", expires_at: "2026-08-26T16:00:00Z" }
            : null,
    });
}

function listingFixture(includeBuiltin = false) {
    return {
        path: "/",
        files: 1,
        aliases: 0,
        bytes: 8,
        entries: [
            ...(includeBuiltin ? [{ kind: "builtin", name: "API", files: null, bytes: 0 }] : []),
            { kind: "site", name: "hello", files: 1, bytes: 8 },
        ],
    };
}

function inventoryFixture() {
    return {
        site: "hello",
        created_at: "2026-08-26T12:00:00Z",
        updated_at: "2026-08-26T12:00:00Z",
        content_revision: 1,
        tree_hash: treeHash(),
        events: [{ kind: "created", at: "2026-08-26T12:00:00Z", files: 0 }],
        files: [{ path: "index.html", hash: `blake3:${"a".repeat(64)}`, size: 8 }],
        aliases: [],
    };
}

function statsFixture() {
    const distribution = {
        min: 8,
        p25: 8,
        median: 8,
        mean: 8,
        p75: 8,
        max: 8,
        iqr: 0,
        stddev: 0,
    };
    return {
        sites: 1,
        files: 1,
        aliases: 0,
        blobs: 1,
        bytes: 8,
        logical_bytes: 8,
        saved_bytes: 0,
        saved_fraction: 0,
        file_sizes: distribution,
        blob_sizes: distribution,
        serving: {
            cache: { hits: 0, misses: 0, evictions: 0 },
            readers: { operations: 0, waits: 0, wait_micros: 0, query_micros: 0 },
        },
    };
}

function undoFixture() {
    return {
        site: "hello",
        entries: [{
            token: "undo-token",
            kind: "put",
            description: "restore previous state",
            created_at: "2026-08-26T12:00:00Z",
            expires_at: "2026-08-26T16:00:00Z",
            remaining_seconds: 14_400,
        }],
    };
}

function expiryFixture(kind = "site", path = null) {
    return {
        target: { site: "hello", path, kind },
        size: 8,
        refreshed_at: "2026-08-26T12:00:00Z",
        own_policy: {
            mode: "relative",
            min_age_seconds: null,
            max_age_seconds: null,
            max_size_bytes: null,
            power: null,
            retention_seconds: 3600,
            expires_at: "2026-08-26T13:00:00Z",
        },
        inherited_caps: [],
        effective_expires_at: "2026-08-26T13:00:00Z",
        remaining_seconds: 3600,
        limited_by: null,
    };
}

function normalizeBody(value) {
    if (Buffer.isBuffer(value)) {
        return value;
    }
    if (value instanceof Uint8Array) {
        return Buffer.from(value);
    }
    if (value instanceof ArrayBuffer) {
        return Buffer.from(value);
    }
    if (typeof value === "string") {
        return Buffer.from(value);
    }
    return Buffer.from(JSON.stringify(value));
}

async function collectBody(request) {
    const chunks = [];
    for await (const chunk of request) {
        chunks.push(Buffer.from(chunk));
    }
    return Buffer.concat(chunks);
}

async function readLedger(path) {
    const source = await readFile(path, "utf8");
    const version = /^version = "([^"]+)"$/m.exec(source)?.[1];
    const revision = /^absolute_revision = ([0-9]+)$/m.exec(source)?.[1];
    const hash = /^source_hash = "([0-9a-f]+)"$/m.exec(source)?.[1];
    assert(version !== undefined && revision !== undefined && hash !== undefined);
    return {
        version,
        absoluteRevision: Number(revision),
        sourceHash: hash,
    };
}

async function selfTest() {
    const artifacts = await locateGeneratedArtifacts();
    const server = await startMockServer(artifacts.fixture);
    server.expect({
        operation: "stats",
        path: "/STATS",
        headers: {},
    });
    const response = await fetch(`${server.origin}/STATS`);
    assert.equal(response.status, 200);
    assert.equal(response.headers.get("Symbol-API-Version"), artifacts.fixture.api_version);
    assert.equal((await response.json()).sites, 1);
    await server.stop();
    console.log("mock server self-test: ok");
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? "").href) {
    await selfTest();
}
