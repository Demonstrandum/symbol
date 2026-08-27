import assert from "node:assert/strict";
import { webcrypto } from "node:crypto";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import {
    importGeneratedEsm,
    locateGeneratedArtifacts,
    startTemporarySymbol,
} from "../mock-server.mjs";

if (globalThis.crypto === undefined) {
    Object.defineProperty(globalThis, "crypto", { value: webcrypto });
}

const root = resolve(process.cwd());
const artifacts = await locateGeneratedArtifacts(root);
const sdk = await importGeneratedEsm(artifacts.apiJs);
const dataRoot = await mkdtemp(join(tmpdir(), "symbol-sdk-js-real-"));
const binary = resolve(root, process.env.SYMBOL_BIN ?? "target/debug/symbol");
const server = await startTemporarySymbol({ binary, dataRoot, root });
const origin = server.origin;

try {
    const symbol = new sdk.SymbolClient({ origin });
    const initial = await symbol.stats();
    assert.equal(initial.sites, 0);

    const site = symbol.site("sdk-real");
    const created = await site.put("<h1>SDK</h1>", {
        mediaType: sdk.MediaTypes.Html,
    });
    assert.equal(created.status, 201);
    assert.equal(created.changed, true);
    assert.equal(created.undo.token.length > 0, true);

    const inventory = await site.files();
    assert.equal(inventory.status, 200);
    assert.equal(inventory.site, "sdk-real");
    assert.equal(inventory.files.some((file) => file.path === "index.html"), true);
    assert.equal(await site.file("index.html").text(), "<h1>SDK</h1>");

    const allocated = await site.folder("generated").text("allocated text", {
        name: { prefix: "note-", suffix: "-final", extension: ".TXT" },
    });
    assert.equal(allocated.status, 201);
    assert.match(allocated.path, /^generated\/note-[0-9a-f]{64}-final\.txt$/);
    assert.equal(allocated.naming.mode, "generated");
    assert.equal(allocated.blobUrl.pathname.startsWith("/.blob/sdk-real/"), true);

    const alias = await site.alias("latest", allocated.path);
    assert.equal(alias.target, allocated.path);
    assert.equal(await site.file("latest").text(), "allocated text");
    assert.equal(await site.file("latest").hash(), allocated.hash.replace(/^blake3:/, ""));

    const replaced = await site.file(allocated.path).replace("replacement text", {
        baseHash: allocated.hash,
    });
    assert.equal(replaced.outcome, "relocated");
    assert.equal(replaced.relocated, true);
    assert.notEqual(replaced.newPath, replaced.oldPath);

    const spliced = await site.file(replaced.newPath).splice({
        offset: 0,
        deleteBytes: 0,
        insert: new TextEncoder().encode("prefix "),
    }, {
        baseHash: replaced.newHash,
    });
    assert.equal(spliced.changed, true);
    assert.equal(
        await site.file(spliced.newPath).text(),
        "prefix replacement text",
    );

    const expiry = await site.file(spliced.newPath).setExpiry({
        mode: "relative",
        duration: "1h",
    });
    assert.equal(expiry.report.target.kind, "file");
    assert(expiry.report.effectiveExpiresAt instanceof Date);

    const generated = await site.files("generated");
    assert.equal(generated.status, 200);
    assert.equal(generated.entries.length >= 1, true);

    const archive = await site.archive("tar.gz");
    assert.equal((await archive.blob()).size > 0, true);
    await archive[Symbol.asyncDispose]();

    const popped = await site.remove();
    assert.equal(popped.undo.token.length > 0, true);
    await popped[Symbol.asyncDispose]();
    const restored = await site.undo(popped.undo.token);
    assert(restored.restoredAt instanceof Date);
    const restoredInventory = await site.files();
    assert.equal(restoredInventory.status, 200);
    assert.equal(restoredInventory.site, "sdk-real");

    assert.deepEqual(symbol.assertExactApi(), {
        apiVersion: sdk.API_VERSION_PARTS,
        absoluteRevision: sdk.API_REVISION,
        sourceHash: sdk.SOURCE_HASH,
    });
    console.log("SDK JavaScript real temporary workflow: ok");
} catch (error) {
    throw new Error(`${error instanceof Error ? error.stack : String(error)}\n${server.log}`);
} finally {
    await server.stop();
    await rm(dataRoot, { recursive: true, force: true });
}
