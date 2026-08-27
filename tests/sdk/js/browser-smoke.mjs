import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { mkdir, mkdtemp, readFile, rm } from "node:fs/promises";
import { createServer } from "node:http";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
    locateGeneratedArtifacts,
} from "../mock-server.mjs";

const artifacts = await locateGeneratedArtifacts();
const chromium = process.env.CHROMIUM_BIN;

if (chromium === undefined) {
    throw new Error("CHROMIUM_BIN is required; run the canonical nix flake check");
} else {
    const api = await readFile(artifacts.apiJs);
    const globalApi = await readFile(artifacts.apiGlobalJs);
    const html = Buffer.from(`<!doctype html>
<meta charset="utf-8">
<pre id="result">pending</pre>
<script>window.__symbolBuiltin = window.Symbol;</script>
<script src="/symbol.global.js"></script>
<script type="module">
import * as API from "/symbol.js";
const result = document.querySelector("#result");
try {
  if (window.Symbol !== window.__symbolBuiltin) throw new Error("built-in Symbol overwritten");
  if (typeof API.SymbolClient !== "function") throw new Error("ESM SymbolClient missing");
  if (typeof window.SymbolAPI?.SymbolClient !== "function") throw new Error("UMD SymbolClient missing");
  const esm = Object.keys(API).sort().join(",");
  const umd = Object.keys(window.SymbolAPI).sort().join(",");
  if (esm !== umd) throw new Error("ESM/UMD export drift");
  if (API.MediaTypes.Json.toString() !== "application/json") throw new Error("media registry failed");
  const retry = {
    maxAttempts: 2, initialDelayMs: 0, maximumDelayMs: 0, backoffFactor: 1,
    jitter: "none", honorRetryAfter: false, retryNetworkErrors: false, retryStatuses: [500],
  };
  const client = new API.SymbolClient({ origin: location.origin, retryPolicy: retry });
  const statsOperation = client.stats();
  const stats = await statsOperation;
  if (stats.sites !== 1 || statsOperation.attempts.length !== 2) throw new Error("browser retry failed");
  const put = await client.site("hello").put("browser");
  if (put.status !== 201 || put.site !== "hello") throw new Error("browser site PUT failed");
  const raw = await client.request("/stream");
  const reader = raw.body.getReader();
  await reader.read();
  await raw[Symbol.asyncDispose]();
  for (let attempt = 0; attempt < 50; attempt++) {
    if (await (await fetch("/disposed-check")).text() === "true") break;
    await new Promise(resolve => setTimeout(resolve, 10));
  }
  if (await (await fetch("/disposed-check")).text() !== "true") throw new Error("browser disposal failed");
  const umdStats = await new window.SymbolAPI.SymbolClient({ origin: location.origin }).stats();
  if (umdStats.sites !== 1) throw new Error("UMD representative method failed");
  result.textContent = "pass";
} catch (error) {
  result.textContent = "fail: " + (error.operation ?? "browser") + ": " + error.message;
}
</script>
`);
    let statsCalls = 0;
    let streamDisposed = false;
    const identity = {
        "Symbol-API-Version": artifacts.fixture.api_version,
        "Symbol-API-Revision": String(artifacts.fixture.absolute_revision),
        "Symbol-API-Source-Hash": artifacts.fixture.source_hash,
    };
    const server = createServer((request, response) => {
        if (request.url === "/symbol.js") {
            response.writeHead(200, { "Content-Type": "text/javascript; charset=utf-8" });
            response.end(api);
        } else if (request.url === "/symbol.global.js") {
            response.writeHead(200, { "Content-Type": "text/javascript; charset=utf-8" });
            response.end(globalApi);
        } else if (request.url === "/") {
            response.writeHead(200, { "Content-Type": "text/html; charset=utf-8" });
            response.end(html);
        } else if (request.url === "/STATS") {
            statsCalls += 1;
            if (statsCalls === 1) {
                response.writeHead(500, {
                    ...identity,
                    "Content-Type": "text/plain; charset=utf-8",
                });
                response.end("error: retry\n");
                return;
            }
            response.writeHead(200, {
                ...identity,
                "Content-Type": "application/json",
            });
            response.end(JSON.stringify(statsFixture()));
        } else if (request.url === "/hello" && request.method === "PUT") {
            const location = `http://127.0.0.1:${server.address().port}/hello/`;
            const tree = `blake3:${"2".repeat(64)}`;
            response.writeHead(201, {
                ...identity,
                "Content-Type": "text/plain; charset=utf-8",
                Location: location,
                ETag: `"${tree}"`,
                "Content-Revision": "1",
                "Undo-Token": "browser-undo",
                "Undo-Expires": "2026-08-26T16:00:00Z",
            });
            response.end(`ok hello ${location} (1 files, changed: true)\n`);
        } else if (request.url === "/stream") {
            response.writeHead(200, {
                ...identity,
                "Content-Type": "application/octet-stream",
            });
            response.write("partial");
            response.once("close", () => {
                streamDisposed = true;
            });
        } else if (request.url === "/disposed-check") {
            response.writeHead(200, { "Content-Type": "text/plain" });
            response.end(String(streamDisposed));
        } else {
            response.writeHead(404);
            response.end();
        }
    });
    await new Promise((resolveStart, rejectStart) => {
        server.once("error", rejectStart);
        server.listen(0, "127.0.0.1", resolveStart);
    });
    const address = server.address();
    assert(address !== null && typeof address === "object");
    const profile = await mkdtemp(join(tmpdir(), "symbol-sdk-chromium-"));
    const config = join(profile, "config");
    const cache = join(profile, "cache");
    await mkdir(config);
    await mkdir(cache);
    try {
        const result = await run(chromium, [
            "--headless=new",
            "--no-sandbox",
            "--disable-gpu",
            "--disable-dev-shm-usage",
            "--disable-breakpad",
            "--disable-crash-reporter",
            "--noerrdialogs",
            `--user-data-dir=${profile}`,
            "--virtual-time-budget=3000",
            "--dump-dom",
            `http://127.0.0.1:${address.port}/`,
        ], {
            ...process.env,
            HOME: profile,
            XDG_CACHE_HOME: cache,
            XDG_CONFIG_HOME: config,
        });
        assert.match(result.stdout, /<pre id="result">pass<\/pre>/);
        assert.doesNotMatch(result.stdout, /<pre id="result">fail:/);
        console.log("browser SDK smoke (headless Chromium): ok");
    } finally {
        await rm(profile, { recursive: true, force: true });
        await new Promise((resolveStop, rejectStop) => {
            server.close((error) => error === undefined ? resolveStop() : rejectStop(error));
            server.closeAllConnections();
        });
    }
}

function statsFixture() {
    const distribution = {
        min: 8, p25: 8, median: 8, mean: 8, p75: 8, max: 8, iqr: 0, stddev: 0,
    };
    return {
        sites: 1, files: 1, aliases: 0, blobs: 1, bytes: 8,
        logical_bytes: 8, saved_bytes: 0, saved_fraction: 0,
        file_sizes: distribution, blob_sizes: distribution,
        serving: {
            cache: { hits: 0, misses: 0, evictions: 0 },
            readers: { operations: 0, waits: 0, wait_micros: 0, query_micros: 0 },
        },
    };
}

async function run(command, arguments_, environment) {
    const child = spawn(command, arguments_, {
        env: environment,
        stdio: ["ignore", "pipe", "pipe"],
    });
    let stdout = "";
    let stderr = "";
    child.stdout.on("data", (chunk) => {
        stdout += chunk.toString();
    });
    child.stderr.on("data", (chunk) => {
        stderr += chunk.toString();
    });
    const code = await new Promise((resolveExit, rejectExit) => {
        child.once("error", rejectExit);
        child.once("exit", resolveExit);
    });
    if (code !== 0) {
        throw new Error(`Chromium exited ${code}\n${stdout}${stderr}`);
    }
    return { stdout, stderr };
}
