// Semantic regression guard for the Excalidraw exporter.
//
// The Rust byte-golden tests assert the exporter's *output* does not change.
// This layer asserts something the byte goldens cannot: that the output actually
// *draws correctly in Excalidraw*. Each committed `.excalidraw` golden is run
// through Excalidraw's own `exportToSvg` and checked for these invariants:
//
//   1. It renders without throwing (a broken binding or malformed element makes
//      the exporter throw).
//   2. Every text string the exporter emitted appears in the rendered SVG — this
//      catches dropped labels, including bound-text/arrow-label loss caused by a
//      regression in the containerId / boundElements wiring.
//   3. The rendered viewBox is non-degenerate (nothing collapsed the scene).
//
// Assertions are deliberately semantic, not pixel/snapshot: rough.js draws with
// per-element seeds and the exact path data shifts between Excalidraw versions,
// so a byte snapshot would be brittle without adding real coverage.

import { readdir } from "node:fs/promises";
import assert from "node:assert/strict";
import test from "node:test";
import { GOLDEN_DIR, loadGolden, renderGolden } from "./render.mjs";

const names = (await readdir(GOLDEN_DIR))
  .filter((f) => f.endsWith(".excalidraw"))
  .map((f) => f.replace(/\.excalidraw$/, ""))
  .sort();

assert.ok(names.length > 0, "expected at least one .excalidraw golden");

for (const name of names) {
  test(`${name}: renders and preserves every label`, async () => {
    const data = await loadGolden(name);
    const expectedTexts = data.elements
      .filter((e) => e.type === "text" && !e.isDeleted && e.text)
      .map((e) => e.text);

    const svg = await renderGolden(name);

    // (1) produced something SVG-shaped
    assert.match(svg, /<svg[\s>]/, "output is not an <svg>");

    // (3) non-degenerate viewBox
    const vb = svg.match(/viewBox="([^"]+)"/);
    assert.ok(vb, "rendered SVG has no viewBox");
    const [, , w, h] = vb[1].split(/\s+/).map(Number);
    assert.ok(w > 0 && h > 0, `degenerate viewBox: ${vb[1]}`);

    // (2) every emitted label is present in the render
    const rendered = [...svg.matchAll(/<text[^>]*>([^<]*)<\/text>/g)]
      .map((m) => m[1])
      .join("\n");
    for (const t of expectedTexts) {
      assert.ok(
        rendered.includes(t),
        `label ${JSON.stringify(t)} from ${name}.excalidraw did not render`,
      );
    }
  });
}
