// Render a committed `.excalidraw` golden through Excalidraw's own engine.
//
// `renderGolden(name)` returns the SVG string that `exportToSvg` produces for
// `tests/golden/<name>.excalidraw` — the same rendering path as excalidraw.com
// (rough.js hand-drawn strokes). The web-font (Virgil) cannot be embedded in a
// headless environment and falls back to a system font, but element geometry,
// arrows and label text are identical to the app.

import "./dom-env.mjs"; // must run before importing the bundle (installs globals)
import { readFile } from "node:fs/promises";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { dom } from "./dom-env.mjs";

const here = dirname(fileURLToPath(import.meta.url));
export const GOLDEN_DIR = join(here, "..", "golden");

let _exportToSvg;
async function getExporter() {
  if (!_exportToSvg) {
    const mod = await import("./dist/excalidraw.bundle.mjs");
    _exportToSvg = mod.exportToSvg;
  }
  return _exportToSvg;
}

/** Parse a golden file into its Excalidraw scene object. */
export async function loadGolden(name) {
  const raw = await readFile(join(GOLDEN_DIR, `${name}.excalidraw`), "utf8");
  return JSON.parse(raw);
}

/** Render `<name>.excalidraw` to an SVG string via Excalidraw's exporter. */
export async function renderGolden(name) {
  const data = await loadGolden(name);
  const exporter = await getExporter();
  const svg = await exporter({
    elements: data.elements,
    appState: {
      ...data.appState,
      exportBackground: true,
      exportWithDarkMode: false,
    },
    files: data.files || {},
  });
  return svg.outerHTML || new dom.window.XMLSerializer().serializeToString(svg);
}
