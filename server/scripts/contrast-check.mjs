#!/usr/bin/env bun
// LC-541: WCAG contrast verifier for the six-palette theme system.
//
// Parses `server/assets/main.css`, resolves each palette x mode combo to its
// varying token block, and checks three foreground/background pairs per combo:
//   - content / surface
//   - content-muted / surface
//   - accent-content / accent
// hc-light / hc-dark require AAA (>= 7.0); light / dark require AA (>= 4.5).
// Prints a row per combo+pair and exits non-zero if any pair fails its
// threshold.
//
// Run from the repo root: `bun server/scripts/contrast-check.mjs`.

import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";

const HERE = dirname(fileURLToPath(import.meta.url));
const CSS_PATH = resolve(HERE, "..", "assets", "main.css");
// Strip block comments before parsing: several token-block comments embed
// literal braces (e.g. `:root{...}` and `.lc-sidebar { ... }`) that would
// otherwise break the naive brace-balanced block matcher below.
const css = readFileSync(CSS_PATH, "utf8").replace(/\/\*[\s\S]*?\*\//g, "");

const PALETTES = [
  "blue-harbor",
  "cobalt",
  "ink-ice",
  "arctic",
  "deep-sea",
  "royal-navy",
  "amethyst",
];
const MODES = ["light", "dark", "hc-light", "hc-dark"];

// --- CSS block extraction -------------------------------------------------

function escapeRe(s) {
  return s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

// Return the merged `--name: value` map for every block whose selector is
// EXACTLY `selector`. The trailing `\s*\{` guards against a bare selector
// matching a compound one (e.g. `[data-theme="cobalt"]` vs
// `[data-theme="cobalt"][data-mode="dark"]`). Later declarations win, but the
// two `:root` blocks define disjoint vars so the merge is a union.
function blockDecls(selector) {
  const re = new RegExp(escapeRe(selector) + "\\s*\\{([^{}]*)\\}", "g");
  const out = {};
  let m;
  let found = false;
  while ((m = re.exec(css)) !== null) {
    found = true;
    const body = m[1];
    const declRe = /(--[\w-]+)\s*:\s*([^;]+);/g;
    let d;
    while ((d = declRe.exec(body)) !== null) {
      out[d[1]] = d[2].trim();
    }
  }
  if (!found) throw new Error(`no CSS block found for selector: ${selector}`);
  return out;
}

// Merged :root (brand vars + Blue Harbor light tokens) backs var() resolution.
const rootVars = blockDecls(":root");

// Resolve a token value, following `var(--x)` references through the block's
// own vars first, then :root. Depth-guarded against accidental cycles.
function resolveValue(value, blockVars, depth = 0) {
  if (depth > 10) throw new Error(`var() resolution too deep for: ${value}`);
  const m = value.match(/^var\(\s*(--[\w-]+)\s*\)$/);
  if (!m) return value.trim();
  const name = m[1];
  const next = (blockVars && blockVars[name]) ?? rootVars[name];
  if (next == null) throw new Error(`unresolved var reference: ${name}`);
  return resolveValue(next, blockVars, depth + 1);
}

// The selector that carries a palette x mode combo's varying tokens.
function selectorFor(palette, mode) {
  if (mode === "light") {
    return palette === "blue-harbor" ? ":root" : `[data-theme="${palette}"]`;
  }
  return `[data-theme="${palette}"][data-mode="${mode}"]`;
}

// --- color math (WCAG 2.x) ------------------------------------------------

function parseHex(hex) {
  let h = hex.trim().replace(/^#/, "");
  if (h.length === 3) {
    h = h
      .split("")
      .map((c) => c + c)
      .join("");
  }
  if (!/^[0-9a-fA-F]{6}$/.test(h)) {
    throw new Error(`not a hex color: ${hex}`);
  }
  return [
    parseInt(h.slice(0, 2), 16),
    parseInt(h.slice(2, 4), 16),
    parseInt(h.slice(4, 6), 16),
  ];
}

function channelLinear(c8) {
  const c = c8 / 255;
  return c <= 0.03928 ? c / 12.92 : Math.pow((c + 0.055) / 1.055, 2.4);
}

function relLuminance(hex) {
  const [r, g, b] = parseHex(hex);
  return (
    0.2126 * channelLinear(r) +
    0.7152 * channelLinear(g) +
    0.0722 * channelLinear(b)
  );
}

function contrastRatio(fg, bg) {
  const lf = relLuminance(fg);
  const lb = relLuminance(bg);
  const hi = Math.max(lf, lb);
  const lo = Math.min(lf, lb);
  return (hi + 0.05) / (lo + 0.05);
}

// --- run ------------------------------------------------------------------

const PAIRS = [
  ["content", "surface"],
  ["content-muted", "surface"],
  ["accent-content", "accent"],
];

const rows = [];
let failCount = 0;

for (const palette of PALETTES) {
  for (const mode of MODES) {
    const isHc = mode === "hc-light" || mode === "hc-dark";
    const threshold = isHc ? 7.0 : 4.5;
    const vars = blockDecls(selectorFor(palette, mode));
    // For a non-Blue-Harbor light combo the block only carries the varying
    // tokens; every one we read (content / content-muted / surface / accent /
    // accent-content) is defined there. Blue Harbor light reads from :root.
    const read = (name) => {
      const raw = vars[`--${name}`];
      if (raw == null) {
        throw new Error(`missing --${name} in ${selectorFor(palette, mode)}`);
      }
      return resolveValue(raw, vars);
    };
    for (const [fgName, bgName] of PAIRS) {
      const fg = read(fgName);
      const bg = read(bgName);
      const ratio = contrastRatio(fg, bg);
      const pass = ratio >= threshold;
      if (!pass) failCount++;
      rows.push({
        palette,
        mode,
        pair: `${fgName}/${bgName}`,
        fg,
        bg,
        ratio,
        threshold,
        pass,
      });
    }
  }
}

// --- report ---------------------------------------------------------------

const pad = (s, n) => String(s).padEnd(n);
const padL = (s, n) => String(s).padStart(n);

console.log(
  pad("PALETTE", 12) +
    pad("MODE", 10) +
    pad("PAIR", 26) +
    pad("FG", 10) +
    pad("BG", 10) +
    padL("RATIO", 8) +
    padL("MIN", 6) +
    "  RESULT",
);
console.log("-".repeat(92));
for (const r of rows) {
  console.log(
    pad(r.palette, 12) +
      pad(r.mode, 10) +
      pad(r.pair, 26) +
      pad(r.fg, 10) +
      pad(r.bg, 10) +
      padL(r.ratio.toFixed(2), 8) +
      padL(r.threshold.toFixed(1), 6) +
      "  " +
      (r.pass ? "PASS" : "FAIL"),
  );
}
console.log("-".repeat(92));
const total = rows.length;
console.log(
  `${total} checks: ${total - failCount} pass, ${failCount} fail ` +
    `(${PALETTES.length} palettes x ${MODES.length} modes x ${PAIRS.length} pairs)`,
);

if (failCount > 0) {
  console.log("\nFAILURES:");
  for (const r of rows.filter((x) => !x.pass)) {
    console.log(
      `  ${r.palette} ${r.mode} ${r.pair}: ${r.ratio.toFixed(2)} ` +
        `(need >= ${r.threshold.toFixed(1)}) fg=${r.fg} bg=${r.bg}`,
    );
  }
}

process.exit(failCount > 0 ? 1 : 0);
