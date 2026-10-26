import { describe, expect, it } from "vitest";

// macOS 12's WebKit ignores these properties without the -webkit- prefix (user-select
// unprefixed means text highlights while dragging in the UI).
const NEEDS_PREFIX = ["user-select", "backdrop-filter", "text-size-adjust", "line-clamp", "box-decoration-break"];

const sheets = import.meta.glob("../**/*.css", { query: "?raw", import: "default", eager: true }) as Record<
  string,
  string
>;

/** Each `selector { … }` body, without comments. */
function blocks(css: string): string[] {
  return [...css.replace(/\/\*[\s\S]*?\*\//g, "").matchAll(/\{([^{}]*)\}/g)].map((m) => m[1]);
}

/** Unprefixed properties in `css` that lack a -webkit- twin in the same block. */
export function missingPrefixes(css: string): string[] {
  const missing: string[] = [];
  for (const body of blocks(css)) {
    for (const prop of NEEDS_PREFIX) {
      const plain = new RegExp(`(^|[;\\s])${prop}\\s*:`).test(body);
      const prefixed = new RegExp(`-webkit-${prop}\\s*:`).test(body);
      if (plain && !prefixed) missing.push(prop);
    }
  }
  return missing;
}

describe("stylesheets work on macOS 12 WebKit", () => {
  it("finds the stylesheets", () => {
    expect(Object.keys(sheets).length).toBeGreaterThan(3);
  });

  it.each(Object.keys(sheets))("%s has -webkit- prefixes", (path) => {
    expect(missingPrefixes(sheets[path])).toEqual([]);
  });

  it("catches a missing prefix", () => {
    expect(missingPrefixes(".a { user-select: none; }")).toEqual(["user-select"]);
    expect(missingPrefixes(".a { -webkit-user-select: none; user-select: none; }")).toEqual([]);
  });
});
