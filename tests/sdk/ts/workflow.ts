import { SymbolClient } from "./api.js";

declare const process: {
    readonly argv: readonly string[];
};

const origin = process.argv[2];
if (origin === undefined) {
    throw new Error("workflow origin argument is required");
}

const symbol = new SymbolClient({ origin });
const stats = await symbol.stats();
const receipt = await symbol.site("ts-workflow").put("hello from TypeScript", {
    mediaType: "text/plain; charset=utf-8",
});
await using archive = await symbol.site("ts-workflow").archive();
const archiveSize = (await archive.blob()).size;
const inventory = await symbol.site("ts-workflow").files();

if (receipt.status !== 200 && receipt.status !== 201) {
    throw new Error("unexpected site PUT status");
}
if (inventory.status !== 200) {
    throw new Error(`unexpected inventory status ${inventory.status}`);
}
if (inventory.site !== "ts-workflow" && inventory.site !== "hello") {
    throw new Error("unexpected inventory fixture");
}

console.log(JSON.stringify({
    sites: stats.sites,
    putStatus: receipt.status,
    archiveSize,
    inventoryFiles: inventory.files.length,
}));
