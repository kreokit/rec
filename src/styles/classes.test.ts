import { describe, expect, it } from "vitest";

// Every stylesheet is bundled into one, so a class styled on its own in one stylesheet
// (`.bubble { position: fixed }` for the recording bubble) also styles any element of
// another screen that has that class: Finish's preview bubble (`stage-camera bubble`) was
// drawn over the window and the side panel that way (v0.1.0-alpha draft, 2026-10-28).
// So a class styled on its own belongs to its stylesheet; other stylesheets may not use it.
// The UI kit (ui.css) is shared on purpose.

const sheets = import.meta.glob("../**/*.css", { query: "?raw", import: "default", eager: true }) as Record<
  string,
  string
>;

const SHARED = ["ui.css"];

const name = (path: string) => path.split("/").pop()!;

/** Every selector in `css`, without comments, @-rules' own lines and declarations. */
function selectors(css: string): string[] {
  const plain = css.replace(/\/\*[\s\S]*?\*\//g, "");
  return [...plain.matchAll(/([^{};]+)\{/g)]
    .map((m) => m[1].trim())
    .filter((s) => !s.startsWith("@"))
    .flatMap((s) => s.split(","))
    .map((s) => s.trim())
    .filter(Boolean);
}

/** The classes a selector uses, and the one it styles anywhere: its leftmost part when that
 * is just one class (with pseudo-classes): `.a`, `.a:hover`, `.a img`; not `.b .a`, which
 * only reaches `.a` inside the stylesheet's own `.b`. */
export function classesIn(selector: string): { used: string[]; alone: string[] } {
  const parts = selector.split(/\s*[\s>+~]\s*/).filter(Boolean);
  const used = [...selector.matchAll(/\.([A-Za-z_][\w-]*)/g)].map((m) => m[1]);
  const alone = parts
    .slice(0, 1)
    .map((part) => part.replace(/::?[\w-]+(\([^)]*\))?/g, ""))
    .filter((part) => /^\.[A-Za-z_][\w-]*$/.test(part))
    .map((part) => part.slice(1));
  return { used, alone };
}

/** Classes styled on their own in one stylesheet and used in another: "class (a.css, b.css)". */
export function leaks(files: Record<string, string>): string[] {
  const alone = new Map<string, string>();
  const used = new Map<string, Set<string>>();
  for (const [path, css] of Object.entries(files)) {
    for (const selector of selectors(css)) {
      const c = classesIn(selector);
      if (!SHARED.includes(name(path))) c.alone.forEach((cls) => alone.set(cls, name(path)));
      c.used.forEach((cls) => used.set(cls, (used.get(cls) ?? new Set()).add(name(path))));
    }
  }
  const out: string[] = [];
  for (const [cls, owner] of alone) {
    const others = [...(used.get(cls) ?? [])].filter((f) => f !== owner);
    if (others.length) out.push(`${cls} (${owner}, ${others.join(", ")})`);
  }
  return out.sort();
}

describe("stylesheets don't style other screens", () => {
  it("finds the stylesheets", () => {
    expect(Object.keys(sheets).length).toBeGreaterThan(3);
  });

  it("no class styled on its own in one stylesheet is used in another", () => {
    expect(leaks(sheets)).toEqual([]);
  });

  it("catches the leak that put Finish's bubble over the window", () => {
    expect(
      leaks({
        "bubble.css": ".bubble { position: fixed; inset: 0 }\n.bubble img { width: 100% }",
        "finish.css": ".stage-camera { position: absolute }\n.stage-camera.bubble { cursor: move }",
      }),
    ).toEqual(["bubble (bubble.css, finish.css)"]);
  });

  it("allows modifiers and the UI kit", () => {
    expect(
      leaks({
        "control.css": ".control-dot.paused { opacity: 0.5 }",
        "recording.css": ".rec-state.paused .rec-dot { opacity: 0.5 }",
        "ui.css": ".ui-button { padding: 0 }",
        "home.css": ".record-button.ui-button { height: 48px }",
        "rec.css": ".rec-controls .ui-button { height: 48px }",
      }),
    ).toEqual([]);
  });
});
