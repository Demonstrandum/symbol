import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import {
    copyFile,
    mkdtemp,
    mkdir,
    readFile,
    rm,
    writeFile,
} from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import {
    locateGeneratedArtifacts,
    startMockServer,
    startTemporarySymbol,
} from "../mock-server.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const root = resolve(here, "../../..");
const artifacts = await locateGeneratedArtifacts(root);
const temporary = await mkdtemp(join(tmpdir(), "symbol-sdk-ts-"));
const compiler = process.env.TSC ?? "tsc";
const compilerOptions = [
    "--strict",
    "--target", "ES2020",
    "--module", "ES2022",
    "--moduleResolution", "Bundler",
    "--lib", "ES2020,DOM,DOM.Iterable,ESNext.Disposable",
    "--skipLibCheck", "false",
];

try {
    await run(compiler, [
        "--noEmit",
        ...compilerOptions,
        artifacts.apiTs,
    ], root);
    console.log("ok 1 - generated api.ts compiles strictly");

    await run(compiler, [
        "--noEmit",
        ...compilerOptions,
        artifacts.apiDeclarations,
    ], root);
    console.log("ok 2 - generated api.d.ts compiles strictly");

    const typeDirectory = join(temporary, "types");
    await mkdir(typeDirectory);
    await copyFile(artifacts.apiDeclarations, join(typeDirectory, "api.d.ts"));
    await copyFile(join(here, "types.ts"), join(typeDirectory, "types.ts"));
    await writeFile(join(typeDirectory, "package.json"), "{\"type\":\"module\"}\n");
    await run(compiler, [
        "--noEmit",
        ...compilerOptions,
        join(typeDirectory, "types.ts"),
    ], typeDirectory);
    console.log("ok 3 - overload, narrowing, cleanup, and negative assertions compile");

    const workflowDirectory = join(temporary, "workflow");
    const outputDirectory = join(workflowDirectory, "dist");
    await mkdir(workflowDirectory);
    await copyFile(artifacts.apiDeclarations, join(workflowDirectory, "api.d.ts"));
    await copyFile(join(here, "workflow.ts"), join(workflowDirectory, "workflow.ts"));
    await writeFile(join(workflowDirectory, "package.json"), "{\"type\":\"module\"}\n");
    await run(compiler, [
        ...compilerOptions,
        "--outDir", outputDirectory,
        join(workflowDirectory, "workflow.ts"),
    ], workflowDirectory);
    await copyFile(artifacts.apiJs, join(outputDirectory, "api.js"));
    await writeFile(join(outputDirectory, "package.json"), "{\"type\":\"module\"}\n");
    const workflow = join(outputDirectory, "workflow.js");
    const bootstrap = join(outputDirectory, "bootstrap.mjs");
    await writeFile(
        bootstrap,
        "import { webcrypto } from 'node:crypto';\n"
        + "if (globalThis.crypto === undefined) "
        + "Object.defineProperty(globalThis, 'crypto', { value: webcrypto });\n"
        + "await import('./workflow.js');\n",
    );
    console.log("ok 4 - runnable TypeScript workflow compiles to ES2020");

    const mock = await startMockServer(artifacts.fixture);
    mock.expect(
        { operation: "stats", path: "/STATS" },
        {
            operation: "site put",
            path: "/ts-workflow",
            body: "hello from TypeScript",
            status: 201,
            headers: { "content-type": "text/plain; charset=utf-8" },
        },
        { operation: "archive get", path: "/ts-workflow.tar.gz" },
        {
            operation: "files inventory",
            path: "/ts-workflow/FILES",
            headers: { accept: "application/json" },
        },
    );
    const mockOutput = JSON.parse((await run(
        process.execPath,
        [bootstrap, mock.origin],
        workflowDirectory,
    )).stdout.trim());
    assert.equal(mockOutput.sites, 1);
    assert.equal(mockOutput.putStatus, 201);
    assert.equal(mockOutput.archiveSize > 0, true);
    await mock.stop();
    console.log("ok 5 - compiled TypeScript workflow passes matching mock");

    const realRoot = join(temporary, "real-store");
    await mkdir(realRoot);
    const binary = resolve(root, process.env.SYMBOL_BIN ?? "target/debug/symbol");
    const server = await startTemporarySymbol({ binary, dataRoot: realRoot, root });
    const origin = server.origin;
    try {
        const realOutput = JSON.parse((await run(
            process.execPath,
            [bootstrap, origin],
            workflowDirectory,
        )).stdout.trim());
        assert.equal(realOutput.putStatus, 201);
        assert.equal(realOutput.archiveSize > 0, true);
        assert.equal(realOutput.inventoryFiles, 1);
    } catch (error) {
        throw new Error(`${error instanceof Error ? error.message : String(error)}\n${server.log}`);
    } finally {
        await server.stop();
    }
    console.log("ok 6 - compiled TypeScript workflow passes real temporary Symbol");
    console.log("SDK TypeScript: 6 tests");
} finally {
    await rm(temporary, { recursive: true, force: true });
}

async function run(command, arguments_, cwd) {
    const child = spawn(command, arguments_, {
        cwd,
        env: process.env,
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
        throw new Error(
            `${command} ${arguments_.join(" ")} exited ${code}\n${stdout}${stderr}`,
        );
    }
    return { stdout, stderr };
}
