// Bundle Excalidraw's `exportToSvg` into a single self-contained ESM file.
//
// The published `@excalidraw/excalidraw` build is an ESM bundle that imports a
// `.json` asset without an import attribute, which Node's strict ESM loader
// rejects. esbuild inlines that JSON (and every other dependency) so the result
// runs under plain `node`. The bundle is large (~15 MB) and version-specific, so
// it is gitignored and rebuilt on demand (see `pretest` in package.json).

import { build } from "esbuild";
import { mkdir, writeFile } from "node:fs/promises";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const outfile = join(here, "dist", "excalidraw.bundle.mjs");
const entry = join(here, "dist", "entry.mjs");

await mkdir(join(here, "dist"), { recursive: true });
await writeFile(entry, `export { exportToSvg } from "@excalidraw/excalidraw";\n`);

await build({
  entryPoints: [entry],
  bundle: true,
  platform: "node",
  format: "esm",
  outfile,
  loader: { ".json": "json" },
  logLevel: "error",
});

console.log("built", outfile);
