import assert from "node:assert/strict";
import {
    ScenarioStep,
    importGeneratedEsm,
    loadGeneratedUmd,
    locateGeneratedArtifacts,
    startMockServer,
} from "../mock-server.mjs";

const tests = [];

function test(name, run) {
    tests.push({ name, run });
}

const artifacts = await locateGeneratedArtifacts();
const esm = await importGeneratedEsm(artifacts.apiJs);
const umd = await loadGeneratedUmd(artifacts.apiGlobalJs);

test("ESM and UMD expose identical runtime exports", async () => {
    assert.deepEqual(
        Object.keys(umd.exports).sort(),
        Object.keys(esm).sort(),
    );
    assert.equal(umd.context.Symbol, Symbol);
    assert.equal(umd.context.SymbolAPI, umd.exports);
});

test("media types and content formats are immutable and exact", async () => {
    const parsed = esm.MediaType.parse(
        "Application/Problem+JSON; profile=\"a b\"; Charset=UTF-8",
    );
    assert.equal(parsed.type, "application");
    assert.equal(parsed.subtype, "problem+json");
    assert.equal(parsed.structuredSuffix, "json");
    assert.equal(parsed.charset, "utf-8");
    assert.equal(
        parsed.toString(),
        "application/problem+json; charset=utf-8; profile=\"a b\"",
    );
    assert(parsed.equals("application/problem+json; profile=\"a b\"; charset=UTF-8"));
    assert(Object.isFrozen(parsed));
    assert(Object.isFrozen(parsed.parameters));
    assert(Object.isFrozen(parsed.parameters[0]));
    assert(Object.isFrozen(esm.MediaTypes));
    assert(Object.isFrozen(esm.ContentFormats));
    assert.equal(esm.MediaTypes.Json.toString(), "application/json");
    assert.equal(esm.MediaTypes.Text.toString(), "text/plain; charset=utf-8");
    assert.equal(esm.ContentFormats.Html.preferredExtension, "html");
    assert.deepEqual(esm.ContentFormats.JavaScript.extensions, ["js", "mjs"]);
    for (const vector of artifacts.fixture.extension_normalization) {
        if (vector.output === null) {
            assert.throws(() => esm.normalizeExtension(vector.input), TypeError, vector.input);
        } else {
            assert.equal(esm.normalizeExtension(vector.input), vector.output, vector.input);
        }
    }
    const allocatedHash = "a".repeat(64);
    assert.equal(
        esm.allocatedFileName(allocatedHash, {
            prefix: "pre-",
            suffix: "-final",
            extension: ".ANNO",
        }),
        `pre-${allocatedHash}-final.anno`,
    );
    for (const invalid of [
        "text",
        "text/plain; charset=utf-8; CHARSET=latin1",
        "text/plain; bad name=value",
        "text/plain; x=\"unterminated",
        "text/plain; x=\"bad\u0001\"",
    ]) {
        assert.throws(() => esm.MediaType.parse(invalid), TypeError, invalid);
    }
    for (const invalid of ["", "."]) {
        assert.throws(() => esm.normalizeExtension(invalid), TypeError, invalid);
    }
});

test("all 40 installed endpoint mappings execute exact decoders", async () => {
    const server = await startMockServer(artifacts.fixture);
    const hash = "a".repeat(64);
    server.expect(
        { operation: "docs", path: "/" },
        { operation: "unnamed put", path: "/", body: "new site" },
        { operation: "docs hash", path: "/HASH" },
        { operation: "stats", path: "/STATS" },
        { operation: "installer", path: "/install.sh" },
        { operation: "installer hash", path: "/install.sh/HASH" },
        { operation: "client", path: "/symbol.sh" },
        { operation: "client hash", path: "/symbol.sh/HASH" },
        { operation: "api client asset", path: "/symbol.ts" },
        { operation: "api client hash", path: "/symbol.ts/HASH" },
        { operation: "api documentation", path: "/API/TS" },
        {
            operation: "api version",
            path: "/API/VERSION",
            headers: { accept: "application/json" },
            responseBody: JSON.stringify({
                api_version: esm.API_VERSION,
                absolute_revision: esm.API_REVISION,
                source_hash: esm.SOURCE_HASH,
            }),
        },
        { operation: "site listing", path: "/FILES", headers: { accept: "application/json" } },
        { operation: "site redirect", path: "/hello" },
        { operation: "site index", path: "/hello/" },
        { operation: "site put", path: "/hello", body: "site" },
        { operation: "site pop", path: "/hello" },
        { operation: "site copy", path: "/hello", headers: { destination: "/copy" } },
        { operation: "site move", path: "/hello", headers: { destination: "/moved" } },
        { operation: "site undo", path: "/hello" },
        {
            operation: "site expire",
            path: "/hello",
            headers: { "expiry-mode": "relative", "expiry-in": "1h" },
        },
        {
            operation: "site management",
            path: "/hello",
            headers: { "management-action": "status" },
        },
        { operation: "site file", path: "/hello/data.bin" },
        { operation: "file put", path: "/hello/data.bin", body: "data" },
        { operation: "file delete", path: "/hello/data.bin" },
        {
            operation: "file expire",
            path: "/hello/data.bin",
            headers: { "expiry-mode": "never" },
        },
        { operation: "archive get", path: "/hello.tar.gz" },
        { operation: "archive pop", path: "/hello.zip" },
        {
            operation: "files inventory",
            path: "/hello/FILES",
            headers: { accept: "application/json" },
        },
        {
            operation: "files subtree",
            path: "/hello/FILES/assets/",
            headers: { accept: "application/json" },
        },
        { operation: "file hash", path: "/hello/data.bin/HASH" },
        { operation: "undo stack", path: "/hello/UNDO" },
        { operation: "expiry inventory", path: "/hello/EXPIRES" },
        { operation: "expiry target", path: "/hello/data.bin/EXPIRES" },
        { operation: "immutable blob", path: `/.blob/hello/${hash}` },
        {
            operation: "alias batch",
            path: "/hello/",
            headers: { "content-type": "application/json" },
            body: JSON.stringify({ aliases: [{ path: "latest", target: "data.bin" }] }),
        },
        {
            operation: "alias file",
            path: "/hello/latest",
            headers: { "alias-target": "data.bin" },
        },
        {
            operation: "allocated file",
            path: "/hello/generated/",
            headers: {
                "allocation-action": "create",
                "content-type": "text/plain; charset=utf-8",
                "file-extension": "txt",
            },
            body: "allocated",
            status: 201,
        },
        {
            operation: "file replace",
            path: "/hello/data.bin",
            headers: { "if-content-match": hash },
            body: "replacement",
        },
        {
            operation: "file splice",
            path: "/hello/data.bin",
            headers: {
                "if-content-match": hash,
                splice: "offset=0; delete=0; insert=1",
            },
            body: Buffer.from("x"),
        },
    );

    const client = new esm.SymbolClient({
        origin: server.origin,
        retryPolicy: {
            ...esm.RetryPolicies.Default,
            maxAttempts: 3,
            initialDelayMs: 0,
            maximumDelayMs: 0,
            jitter: "none",
        },
    });
    const site = client.site("hello");
    assert.equal((await client.docs()).status, 200);
    assert.equal((await client.create("new site")).status, 201);
    assert.equal((await client.docsHash()).length, 64);
    assert.equal((await client.stats()).sites, 1);
    assert.equal((await client.installer()).status, 200);
    assert.equal((await client.installerHash()).length, 64);
    assert.equal((await client.shellClient()).status, 200);
    assert.equal((await client.shellClientHash()).length, 64);
    assert.equal((await client.apiClient("symbol.ts")).status, 200);
    assert.equal((await client.apiClientHash("symbol.ts")).length, 64);
    assert.equal((await client.apiManual("typescript")).status, 200);
    assert.deepEqual((await client.apiVersion()).identity.apiVersion, esm.API_VERSION_PARTS);
    assert.equal((await client.sites()).entries[0].kind, "builtin");
    assert.equal((await site.redirect()).status, 307);
    const index = await site.get();
    assert.equal(index.status, 200);
    await index[Symbol.asyncDispose]();
    assert.equal((await site.put("site")).status, 200);
    const removedSite = await site.remove();
    assert.equal(removedSite.undo.token, "undo-pop");
    await removedSite[Symbol.asyncDispose]();
    assert.equal((await site.copy("copy")).status, 201);
    assert.equal((await site.move("moved")).status, 200);
    assert((await site.undo()).restoredAt instanceof Date);
    assert.equal(
        (await site.setExpiry({ mode: "relative", duration: "1h" })).report.target.kind,
        "site",
    );
    assert.equal((await site.management().status()).managed, false);
    const hosted = await site.file("data.bin").get();
    assert.equal(await hosted.text(), "abcdefgh");
    await hosted[Symbol.asyncDispose]();
    assert.equal((await site.file("data.bin").put("data")).status, 200);
    assert.equal((await site.file("data.bin").remove()).status, 200);
    assert.equal(
        (await site.file("data.bin").setExpiry({ mode: "never" })).changed,
        true,
    );
    const archive = await site.archive();
    assert.equal(archive.filename, "hello.tar.gz");
    await archive[Symbol.asyncDispose]();
    const popped = await site.pop("zip");
    assert.equal(popped.format, "zip");
    await popped[Symbol.asyncDispose]();
    const inventory = await site.files();
    assert.equal(inventory.status, 200);
    assert.equal(inventory.site, "hello");
    const subtree = await site.files("assets");
    assert.equal(subtree.status, 200);
    assert.equal(subtree.entries[0].kind, "site");
    assert.equal((await site.file("data.bin").hash()).length, 64);
    assert.equal((await site.undoStack()).entries[0].kind, "put");
    assert.equal((await site.expiry()).entries.length, 1);
    assert.equal((await site.expiry("data.bin")).target.kind, "file");
    const blob = await client.blob("hello", hash);
    assert.equal(await blob.text(), "abcdefgh");
    await blob[Symbol.asyncDispose]();
    assert.equal(
        (await site.aliases([{ path: "latest", target: "data.bin" }])).aliases.length,
        1,
    );
    assert.equal((await site.alias("latest", "data.bin")).targetKind, "file");
    const allocation = await site.folder("generated").text("allocated");
    assert.equal(allocation.naming.mode, "generated");
    assert.equal(
        (await site.file("data.bin").replace("replacement", { baseHash: hash })).outcome,
        "replaced",
    );
    assert.equal(
        (await site.file("data.bin").splice({
            offset: 0,
            deleteBytes: 0,
            insert: new Uint8Array([120]),
        }, { baseHash: hash })).splices,
        1,
    );
    assert.deepEqual(client.assertExactApi(), {
        apiVersion: esm.API_VERSION_PARTS,
        absoluteRevision: esm.API_REVISION,
        sourceHash: esm.SOURCE_HASH,
    });
    await server.stop();
    assert.equal(server.requests.length, 40);
});

test("segment encoding, credentials, naming, and custom allocation are exact", async () => {
    const server = await startMockServer(artifacts.fixture);
    server.expect(
        {
            operation: "files subtree",
            path: "/hello/FILES/a%20b/%23c/",
            headers: { accept: "application/json" },
        },
        {
            operation: "allocated file",
            path: "/hello/reports/",
            status: 202,
            headers: {
                "allocation-action": "propose",
                authorization: "Bearer memory-only",
                "content-type": "application/pdf",
                "file-extension": "pdf",
            },
            body: "one upload",
        },
        {
            operation: "allocated file",
            path: "/hello/reports/",
            status: 201,
            headers: {
                "allocation-action": "finalize",
                "allocation-token": "allocation-token",
                authorization: "Bearer memory-only",
                "file-name": "report-final.pdf",
            },
            body: Buffer.alloc(0),
        },
    );
    const client = new esm.SymbolClient({ origin: server.origin, token: "memory-only" });
    await client.site("hello").files("a b/#c");
    let callbackCount = 0;
    const operation = client.site("hello").folder("reports").create("one upload", {
        mediaType: esm.MediaTypes.Pdf,
        name: async (proposal) => {
            callbackCount += 1;
            assert.equal(proposal.hash, `blake3:${"f".repeat(64)}`);
            return "report-final.pdf";
        },
    });
    const logicalKey = operation.idempotencyKey;
    assert.match(logicalKey, /^[0-9a-f-]{36}$/);
    const receipt = await operation;
    assert.equal(operation.idempotencyKey, logicalKey);
    assert.equal(receipt.idempotencyKey, logicalKey);
    assert.equal(receipt.naming.mode, "custom");
    assert.equal(callbackCount, 1);
    await server.stop();
    assert.equal(server.requests[1].body.toString(), "one upload");
    assert.equal(server.requests[2].body.byteLength, 0);
    assert.notEqual(
        server.requests[1].headers["idempotency-key"],
        server.requests[2].headers["idempotency-key"],
    );
    assert.equal(server.requests[1].headers["idempotency-key"], `${logicalKey}:propose`);
    assert.equal(server.requests[2].headers["idempotency-key"], `${logicalKey}:finalize`);
});

test("custom allocation callback failure cancels its proposal", async () => {
    const server = await startMockServer(artifacts.fixture);
    server.expect(
        {
            operation: "allocated file",
            path: "/hello/custom/",
            status: 202,
            headers: { "allocation-action": "propose" },
            body: "pending",
        },
        {
            operation: "allocated file",
            path: "/hello/custom/",
            status: 200,
            fault: "drop",
            headers: {
                "allocation-action": "cancel",
                "allocation-token": "allocation-token",
            },
            body: Buffer.alloc(0),
        },
        {
            operation: "allocated file",
            path: "/hello/custom/",
            status: 200,
            headers: {
                "allocation-action": "cancel",
                "allocation-token": "allocation-token",
            },
            body: Buffer.alloc(0),
        },
    );
    const client = new esm.SymbolClient({
        origin: server.origin,
        retryPolicy: {
            ...esm.RetryPolicies.Default,
            maxAttempts: 2,
            initialDelayMs: 0,
            maximumDelayMs: 0,
            jitter: "none",
        },
    });
    let callbacks = 0;
    const operation = client.site("hello").folder("custom").create("pending", {
        name: () => {
            callbacks += 1;
            throw new Error("callback failed");
        },
    });
    const logicalKey = operation.idempotencyKey;
    await assert.rejects(
        Promise.resolve(operation),
        /callback failed/,
    );
    assert.equal(callbacks, 1);
    assert.equal(operation.canRetry, false);
    assert.throws(() => operation.retry(), esm.OperationStateError);
    await server.stop();
    assert.equal(server.requests.length, 3);
    assert.equal(server.requests[1].headers["idempotency-key"], `${logicalKey}:cancel`);
    assert.equal(server.requests[2].headers["idempotency-key"], `${logicalKey}:cancel`);
});

test("site, nested, and management clients merge credential precedence", async () => {
    const server = await startMockServer(artifacts.fixture);
    server.expect(
        {
            operation: "site put",
            path: "/hello",
            headers: { authorization: "Bearer site-token" },
            body: "site",
        },
        {
            operation: "alias file",
            path: "/hello/latest",
            headers: {
                authorization: "Bearer site-token",
                "alias-target": "data.bin",
            },
        },
        {
            operation: "file replace",
            path: "/hello/data.bin",
            headers: {
                authorization: "Bearer site-token",
                "if-content-match": "a".repeat(64),
            },
            body: "replacement",
        },
        {
            operation: "site management",
            path: "/hello",
            headers: {
                authorization: "Bearer method-token",
                "management-action": "status",
            },
        },
    );
    const site = new esm.SymbolClient({
        origin: server.origin,
        token: "client-token",
    }).site("hello", { token: "site-token" });
    await site.put("site");
    await site.alias("latest", "data.bin");
    await site.file("data.bin").replace("replacement", { baseHash: "a".repeat(64) });
    await site.management().status({ token: "method-token" });
    await server.stop();
});

test("two-phase retry resumes finalization without re-upload or callback replay", async () => {
    const server = await startMockServer(artifacts.fixture);
    server.expect(
        {
            operation: "allocated file",
            path: "/hello/resume/",
            status: 202,
            headers: { "allocation-action": "propose" },
            body: "upload exactly once",
        },
        {
            operation: "allocated file",
            path: "/hello/resume/",
            status: 201,
            fault: "drop",
            headers: {
                "allocation-action": "finalize",
                "allocation-token": "allocation-token",
                "file-name": "chosen.txt",
            },
            body: Buffer.alloc(0),
        },
        {
            operation: "allocated file",
            path: "/hello/resume/",
            status: 201,
            headers: {
                "allocation-action": "finalize",
                "allocation-token": "allocation-token",
                "file-name": "chosen.txt",
            },
            body: Buffer.alloc(0),
        },
    );
    let callbacks = 0;
    const client = new esm.SymbolClient({
        origin: server.origin,
        retryPolicy: {
            ...esm.RetryPolicies.Default,
            maxAttempts: 2,
            initialDelayMs: 0,
            maximumDelayMs: 0,
            jitter: "none",
        },
    });
    const stream = new ReadableStream({
        start(controller) {
            controller.enqueue(new TextEncoder().encode("upload exactly once"));
            controller.close();
        },
    });
    let operation;
    operation = client.site("hello").folder("resume").create(stream, {
        mediaType: esm.MediaTypes.Text,
        name: () => {
            callbacks += 1;
            assert.equal(operation.replayable, true);
            return "chosen.txt";
        },
    });
    assert.equal(operation.replayable, false);
    const logicalKey = operation.idempotencyKey;
    const receipt = await operation;
    assert.equal(receipt.status, 201);
    assert.equal(callbacks, 1);
    await server.stop();
    assert.equal(server.requests.length, 3);
    assert.equal(server.requests[0].body.toString(), "upload exactly once");
    assert.equal(server.requests[1].body.byteLength, 0);
    assert.equal(server.requests[2].body.byteLength, 0);
    assert.equal(server.requests[0].headers["idempotency-key"], `${logicalKey}:propose`);
    assert.equal(server.requests[1].headers["idempotency-key"], `${logicalKey}:finalize`);
    assert.equal(server.requests[2].headers["idempotency-key"], `${logicalKey}:finalize`);
});

test("automatic and manual retries retain one idempotency key and attempt history", async () => {
    const server = await startMockServer(artifacts.fixture);
    server.expect(
        {
            operation: "alias file",
            path: "/hello/current",
            fault: "drop",
            headers: { "idempotency-key": /^[0-9a-f-]{36}$/ },
        },
        {
            operation: "alias file",
            path: "/hello/current",
            headers: { "idempotency-key": /^[0-9a-f-]{36}$/ },
        },
        {
            operation: "alias file",
            path: "/hello/manual",
            fault: "drop",
            headers: { "idempotency-key": "manual-key" },
        },
        {
            operation: "alias file",
            path: "/hello/manual",
            headers: { "idempotency-key": "manual-key" },
        },
    );
    const policy = {
        ...esm.RetryPolicies.Default,
        initialDelayMs: 0,
        maximumDelayMs: 0,
        jitter: "none",
    };
    const automatic = new esm.SymbolClient({
        origin: server.origin,
        retryPolicy: policy,
    }).site("hello").alias("current", "data.bin");
    policy.maxAttempts = 1;
    await automatic;
    assert.equal(automatic.attempts.length, 2);
    assert(Object.isFrozen(automatic.attempts));
    assert(Object.isFrozen(automatic.attempts[0]));
    const startedAt = automatic.attempts[0].startedAt.getTime();
    assert.throws(() => automatic.attempts[0].startedAt.setTime(0), TypeError);
    assert.equal(automatic.attempts[0].startedAt.getTime(), startedAt);

    const manual = new esm.SymbolClient({ origin: server.origin })
        .site("hello")
        .alias("manual", "data.bin", { idempotencyKey: "manual-key" });
    await assert.rejects(Promise.resolve(manual));
    assert.equal(manual.canRetry, true);
    await manual.retry({
        ...esm.RetryPolicies.Default,
        maxAttempts: 1,
        initialDelayMs: 0,
        maximumDelayMs: 0,
        jitter: "none",
    });
    assert.equal(manual.attempts.length, 2);
    assert.throws(() => manual.retry(), esm.OperationStateError);
    await server.stop();
    assert.equal(
        server.requests[0].headers["idempotency-key"],
        server.requests[1].headers["idempotency-key"],
    );

    const wrappedServer = await startMockServer(artifacts.fixture);
    wrappedServer.expect({ operation: "stats", path: "/STATS" });
    let wrappedCalls = 0;
    const wrappedFetch = async (...arguments_) => {
        wrappedCalls += 1;
        if (wrappedCalls === 1) {
            throw new TypeError("injected fetch network failure");
        }
        return globalThis.fetch(...arguments_);
    };
    const wrappedClient = new esm.SymbolClient({
        origin: wrappedServer.origin,
        fetch: wrappedFetch,
        retryPolicy: {
            ...esm.RetryPolicies.Default,
            maxAttempts: 2,
            initialDelayMs: 0,
            maximumDelayMs: 0,
            jitter: "none",
        },
    });
    assert.equal((await wrappedClient.stats()).sites, 1);
    assert.equal(wrappedCalls, 2);
    await wrappedServer.stop();
});

test("Retry-After accepts only integer seconds or RFC HTTP dates", async () => {
    const cases = [
        { value: "1.5", expected: (delays) => assert.deepEqual(delays, []) },
        { value: "1", expected: (delays) => assert.deepEqual(delays, [1_000]) },
        {
            value: new Date(Date.now() + 5_000).toUTCString(),
            expected: (delays) => {
                assert.equal(delays.length, 1);
                assert(delays[0] >= 3_000 && delays[0] <= 5_000);
            },
        },
    ];
    const originalSetTimeout = globalThis.setTimeout;
    try {
        for (const scenario of cases) {
            const delays = [];
            globalThis.setTimeout = (callback, delay) => {
                delays.push(delay);
                queueMicrotask(callback);
                return 0;
            };
            let calls = 0;
            const operation = new esm.SymbolClient({
                origin: "https://retry.invalid",
                fetch: async () => {
                    calls += 1;
                    const identity = {
                        "Symbol-API-Version": esm.API_VERSION,
                        "Symbol-API-Revision": String(esm.API_REVISION),
                        "Symbol-API-Source-Hash": esm.SOURCE_HASH,
                    };
                    if (calls === 1) {
                        return new Response("error: retry\n", {
                            status: 500,
                            headers: {
                                ...identity,
                                "Content-Type": "text/plain; charset=utf-8",
                                "Retry-After": scenario.value,
                            },
                        });
                    }
                    return new Response(JSON.stringify(statsBody()), {
                        status: 200,
                        headers: {
                            ...identity,
                            "Content-Type": "application/json",
                        },
                    });
                },
                retryPolicy: {
                    ...esm.RetryPolicies.Default,
                    maxAttempts: 2,
                    initialDelayMs: 0,
                    maximumDelayMs: 10_000,
                    jitter: "none",
                },
            }).stats();
            await operation;
            scenario.expected(delays);
            assert.equal(calls, 2);
        }
    } finally {
        globalThis.setTimeout = originalSetTimeout;
    }
});

function statsBody() {
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

test("non-replayable bodies, abort, and disposal follow ownership rules", async () => {
    const rejecting = new esm.SymbolClient({
        origin: "http://127.0.0.1:1",
        fetch: async () => {
            throw new TypeError("network down");
        },
    });
    const stream = new ReadableStream({
        start(controller) {
            controller.enqueue(new Uint8Array([1]));
            controller.close();
        },
    });
    const operation = rejecting.site("hello").file("data.bin").replace(stream, {
        baseHash: "a".repeat(64),
    });
    await assert.rejects(Promise.resolve(operation), /network down/);
    assert.equal(operation.replayable, false);
    assert.throws(() => operation.retry(), esm.BodyNotReplayableError);

    const server = await startMockServer(artifacts.fixture);
    server.expect({
        operation: "site file",
        path: "/hello/slow.bin",
        delayMs: 100,
    });
    const delayed = new esm.SymbolClient({ origin: server.origin })
        .site("hello")
        .file("slow.bin")
        .get();
    await new Promise((resolve) => setTimeout(resolve, 10));
    delayed.abort("test abort");
    await assert.rejects(Promise.resolve(delayed));
    await delayed[Symbol.asyncDispose]();
    await delayed[Symbol.asyncDispose]();
    await server.stop();
});

test("unsupported PUT mutations never expose or send idempotency keys", async () => {
    const server = await startMockServer(artifacts.fixture);
    server.expect(
        { operation: "site put", path: "/hello", body: "site" },
        { operation: "file put", path: "/hello/data.bin", body: "file" },
    );
    const site = new esm.SymbolClient({ origin: server.origin }).site("hello");
    const sitePut = site.put("site");
    const filePut = site.file("data.bin").put("file");
    assert.equal(sitePut.idempotencyKey, null);
    assert.equal(filePut.idempotencyKey, null);
    await sitePut;
    await filePut;
    await server.stop();
    assert.equal(server.requests[0].headers["idempotency-key"], undefined);
    assert.equal(server.requests[1].headers["idempotency-key"], undefined);

    const failing = new esm.SymbolClient({
        origin: "http://127.0.0.1:1",
        fetch: async () => {
            throw new TypeError("network down");
        },
        retryPolicy: esm.RetryPolicies.Default,
    }).site("hello").put("site");
    await assert.rejects(Promise.resolve(failing), /network down/);
    assert.equal(failing.canRetry, false);
    assert.throws(() => failing.retry(), esm.OperationStateError);
});

test("concurrent disposal removes listeners and cancels partial owned streams once", async () => {
    const consumed = await new esm.SymbolClient({
        origin: "https://consumed.invalid",
        fetch: async () => new Response("fully consumed", {
            status: 200,
            headers: {
                "Symbol-API-Version": esm.API_VERSION,
                "Symbol-API-Revision": String(esm.API_REVISION),
                "Symbol-API-Source-Hash": esm.SOURCE_HASH,
            },
        }),
    }).request("/complete");
    assert.equal(await consumed.text(), "fully consumed");
    await consumed[Symbol.asyncDispose]();
    await consumed[Symbol.asyncDispose]();

    const external = new AbortController();
    const signal = external.signal;
    const add = signal.addEventListener.bind(signal);
    const remove = signal.removeEventListener.bind(signal);
    let linkedListeners = 0;
    Object.defineProperty(signal, "addEventListener", {
        value(type, listener, options) {
            if (type === "abort") {
                linkedListeners += 1;
            }
            return add(type, listener, options);
        },
    });
    Object.defineProperty(signal, "removeEventListener", {
        value(type, listener, options) {
            if (type === "abort") {
                linkedListeners -= 1;
            }
            return remove(type, listener, options);
        },
    });
    let fetchAborts = 0;
    const pending = new esm.SymbolClient({
        origin: "https://dispose.invalid",
        fetch: async (_input, init) => new Promise((_resolve, reject) => {
            init.signal.addEventListener("abort", () => {
                fetchAborts += 1;
                reject(init.signal.reason);
            }, { once: true });
        }),
    }).stats({ signal });
    await Promise.all([
        pending[Symbol.asyncDispose](),
        pending[Symbol.asyncDispose](),
        pending[Symbol.asyncDispose](),
    ]);
    assert.equal(fetchAborts, 1);
    assert.equal(linkedListeners, 0);

    let cancellations = 0;
    const body = new ReadableStream({
        start(controller) {
            controller.enqueue(new Uint8Array([1]));
        },
        cancel() {
            cancellations += 1;
        },
    });
    const raw = await new esm.SymbolClient({
        origin: "https://dispose.invalid",
        fetch: async () => new Response(body, {
            status: 200,
            headers: {
                "Symbol-API-Version": esm.API_VERSION,
                "Symbol-API-Revision": String(esm.API_REVISION),
                "Symbol-API-Source-Hash": esm.SOURCE_HASH,
            },
        }),
    }).request("/partial");
    const reader = raw.body.getReader();
    await reader.read();
    await Promise.all([
        raw[Symbol.asyncDispose](),
        raw[Symbol.asyncDispose](),
    ]);
    assert.equal(raw.bodyUsed, true);
    assert.equal(cancellations, 1);

    let archiveCancellations = 0;
    const archiveBody = new ReadableStream({
        cancel() {
            archiveCancellations += 1;
        },
    });
    const archiveResponse = new Response(archiveBody, {
        status: 200,
        headers: {
            "Content-Type": "application/gzip",
            "Content-Disposition": "attachment; filename=\"hello.tar.gz\"",
            "Content-Length": "1",
            "Cache-Control": "no-cache",
            "Symbol-API-Version": esm.API_VERSION,
            "Symbol-API-Revision": String(esm.API_REVISION),
            "Symbol-API-Source-Hash": esm.SOURCE_HASH,
        },
    });
    Object.defineProperty(archiveResponse, "blob", {
        value: async () => {
            throw new Error("archive read failed");
        },
    });
    const archive = await new esm.SymbolClient({
        origin: "https://dispose.invalid",
        fetch: async () => archiveResponse,
    }).site("hello").archive();
    await assert.rejects(archive.blob(), /archive read failed/);
    await archive[Symbol.asyncDispose]();
    assert.equal(archiveCancellations, 1);
});

test("header and framed PATCH encodings select at exact limits", async () => {
    const server = await startMockServer(artifacts.fixture);
    const hash = "a".repeat(64);
    server.expect(
        {
            operation: "file splice",
            path: "/hello/data.bin",
            headers: { splice: "offset=0; delete=1; insert=1" },
            body: Buffer.from("x"),
        },
        {
            operation: "file splice",
            path: "/hello/data.bin",
            headers: { splice: /^offset=0; delete=0; insert=0(?:,offset=[0-9]+; delete=0; insert=0){63}$/ },
            body: Buffer.alloc(0),
        },
        {
            operation: "file splice",
            path: "/hello/data.bin",
            headers: {
                "content-type": "application/vnd.symbol.splice; version=1",
            },
            body: (body) => {
                assert.equal(body.subarray(0, 8).toString("binary"), "SYMSPL1\u0000");
                assert.equal(body.readUInt32BE(8), 65);
                assert.equal(body.readUInt32BE(12), 0);
            },
        },
        {
            operation: "file splice",
            path: "/hello/data.bin",
            headers: { splice: "offset=2; delete=0; insert=1" },
            body: Buffer.from("x"),
        },
    );
    const file = new esm.SymbolClient({ origin: server.origin })
        .site("hello")
        .file("data.bin");
    await file.patch([
        { offset: 0, deleteBytes: 1, insert: new Uint8Array([120]) },
    ], { baseHash: hash });
    await file.patch(Array.from({ length: 64 }, (_, index) => ({
        offset: index,
        deleteBytes: 0,
        insert: new Uint8Array(),
    })), { baseHash: hash });
    await file.patch(Array.from({ length: 65 }, (_, index) => ({
        offset: index,
        deleteBytes: 0,
        insert: new Uint8Array(),
    })), { baseHash: hash });
    const insertion = new Uint8Array([120]);
    const descriptor = { offset: 2, deleteBytes: 0, insert: insertion };
    const snapshotted = file.patch([descriptor], { baseHash: hash });
    descriptor.offset = 999;
    descriptor.deleteBytes = 999;
    insertion[0] = 121;
    await snapshotted;
    await server.stop();
});

test("typed HTTP errors preserve response details and malformed DTOs fail closed", async () => {
    const server = await startMockServer(artifacts.fixture);
    server.expect(
        {
            operation: "file replace",
            path: "/hello/data.bin",
            status: 412,
            body: "body",
        },
    );
    const client = new esm.SymbolClient({
        origin: server.origin,
        retryPolicy: {
            ...esm.RetryPolicies.Default,
            maxAttempts: 3,
            initialDelayMs: 0,
            maximumDelayMs: 0,
            jitter: "none",
        },
    });
    const replace = client.site("hello").file("data.bin").replace("body", {
        baseHash: "a".repeat(64),
        idempotencyKey: "replace-key",
    });
    await assert.rejects(Promise.resolve(replace), (error) => {
        assert(error instanceof esm.PreconditionFailedError);
        assert.equal(error.status, 412);
        assert.equal(error.idempotencyKey, "replace-key");
        assert.equal(error.contentRevision, 2);
        assert.match(error.etag, /blake3/);
        return true;
    });
    await server.stop();

    let malformedCalls = 0;
    const malformedClient = new esm.SymbolClient({
        origin: "https://malformed.invalid",
        retryPolicy: {
            ...esm.RetryPolicies.Default,
            maxAttempts: 3,
            initialDelayMs: 0,
            maximumDelayMs: 0,
            jitter: "none",
        },
        fetch: async () => {
            malformedCalls += 1;
            return new Response("{}", {
                status: 200,
                headers: {
                    "Content-Type": "application/json",
                    "Symbol-API-Version": esm.API_VERSION,
                    "Symbol-API-Revision": String(esm.API_REVISION),
                    "Symbol-API-Source-Hash": esm.SOURCE_HASH,
                },
            });
        },
    });
    await assert.rejects(Promise.resolve(malformedClient.stats()), esm.MalformedResponseError);
    assert.equal(malformedCalls, 1);

    const malformedVersion = new esm.SymbolClient({
        origin: "https://malformed-version.invalid",
        fetch: async () => new Response("{}", {
            status: 200,
            headers: {
                "Content-Type": "application/json",
                "Symbol-API-Version": "1.invalid.0",
                "Symbol-API-Revision": String(esm.API_REVISION),
                "Symbol-API-Source-Hash": esm.SOURCE_HASH,
            },
        }),
    });
    await assert.rejects(
        Promise.resolve(malformedVersion.stats()),
        esm.MissingApiIdentityError,
    );
});

test("version compatibility accepts newer peers and rejects impossible identities", async () => {
    const current = artifacts.fixture;
    const identities = [
        {
            identity: {
                apiVersion: `${Number(current.api_version.split(".")[0]) + 1}.0.0`,
                absoluteRevision: current.absolute_revision + 1,
                sourceHash: "1".repeat(64),
            },
            error: esm.IncompatibleApiVersionError,
        },
        {
            identity: {
                apiVersion: current.api_version,
                absoluteRevision: current.absolute_revision,
                sourceHash: "2".repeat(64),
            },
            error: esm.ApiIntegrityError,
        },
        {
            identity: {
                apiVersion: "0.0.9",
                absoluteRevision: 1,
                sourceHash: "3".repeat(64),
            },
            error: esm.OperationUnavailableError,
        },
    ];
    for (const scenario of identities) {
        const server = await startMockServer(current, scenario.identity);
        server.expect({ operation: "stats", path: "/STATS" });
        const client = new esm.SymbolClient({
            origin: server.origin,
            retryPolicy: {
                ...esm.RetryPolicies.Default,
                maxAttempts: 3,
                initialDelayMs: 0,
                maximumDelayMs: 0,
                jitter: "none",
            },
        });
        await assert.rejects(Promise.resolve(client.stats()), scenario.error);
        await server.stop();
        assert.equal(server.requests.length, 1);
    }
    const [major] = current.api_version.split(".").map(Number);
    const newer = await startMockServer(current, {
        apiVersion: `${major}.${Number(current.api_version.split(".")[1]) + 1}.0`,
        absoluteRevision: current.absolute_revision + 1,
        sourceHash: "4".repeat(64),
    });
    newer.expect({ operation: "stats", path: "/STATS" });
    const client = new esm.SymbolClient({ origin: newer.origin });
    assert.equal((await client.stats()).sites, 1);
    assert.throws(() => client.assertExactApi());
    await newer.stop();

    const sessionServer = await startMockServer(current);
    sessionServer.expect(
        { operation: "stats", path: "/STATS" },
        { operation: "stats", path: "/STATS" },
        { operation: "stats", path: "/STATS" },
    );
    const [sessionMajor, sessionMinor, sessionPatch] = current.api_version.split(".").map(Number);
    const upgradedVersion = `${sessionMajor}.${sessionMinor}.${sessionPatch + 1}`;
    let sessionCalls = 0;
    const sessionFetch = async (...arguments_) => {
        sessionCalls += 1;
        const response = await globalThis.fetch(...arguments_);
        if (sessionCalls !== 2) {
            return response;
        }
        const headers = new Headers(response.headers);
        headers.set("Symbol-API-Version", upgradedVersion);
        headers.set("Symbol-API-Revision", String(current.absolute_revision + 1));
        headers.set("Symbol-API-Source-Hash", "5".repeat(64));
        return new Response(response.body, {
            status: response.status,
            statusText: response.statusText,
            headers,
        });
    };
    const sessionClient = new esm.SymbolClient({
        origin: sessionServer.origin,
        fetch: sessionFetch,
    });
    await sessionClient.stats();
    await sessionClient.stats();
    assert.deepEqual(sessionClient.apiIdentity.apiVersion, [
        sessionMajor,
        sessionMinor,
        sessionPatch + 1,
    ]);
    await assert.rejects(Promise.resolve(sessionClient.stats()), esm.ApiIntegrityError);
    await sessionServer.stop();
});

test("UMD client executes independently without replacing Symbol", async () => {
    const server = await startMockServer(artifacts.fixture);
    server.expect({ operation: "stats", path: "/STATS" });
    const client = new umd.exports.SymbolClient({ origin: server.origin });
    assert.equal((await client.stats()).sites, 1);
    assert.equal(umd.context.Symbol, Symbol);
    await server.stop();
});

let passed = 0;
for (const { name, run } of tests) {
    try {
        await run();
        passed += 1;
        console.log(`ok ${passed} - ${name}`);
    } catch (error) {
        console.error(`not ok ${passed + 1} - ${name}`);
        throw error;
    }
}
console.log(`SDK JavaScript runtime: ${passed} tests`);
