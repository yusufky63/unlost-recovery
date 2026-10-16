import { describe, expect, it } from "vitest";
import { reasonParams, recoverable, stateTone, USEFUL_STATES } from "./states";
import { formatBytes, joinInner, driveName, baseName, parentPath } from "./format";
import type { Reason } from "./types";

const ctx = { ext: "jpg", date: (n: number) => `d${n}`, bytes: (n: number) => formatBytes(n) };

describe("states", () => {
  it("maps states to the three status tones only", () => {
    expect(stateTone("likelyIntact")).toBe("positive");
    expect(stateTone("partial")).toBe("warning");
    expect(stateTone("overwritten")).toBe("danger");
    expect(stateTone("metadataOnly")).toBe("neutral");
    expect(USEFUL_STATES).not.toContain("zeroed");
    expect(recoverable("metadataOnly")).toBe(false);
    expect(recoverable("partial")).toBe(true);
  });

  it("fills reason parameters", () => {
    const cases: [Reason, Record<string, string | number>][] = [
      [{ code: "clustersReused", reused: 3, total: 10 }, { reused: 3, total: 10 }],
      [{ code: "headerMismatch", expected: "a JPEG image" }, { ext: "jpg" }],
      [{ code: "shadowCopy", created: 5 }, { date: "d5" }],
      [{ code: "unreadable", bytes: 2048 }, { bytes: "2.0 KB" }],
      [{ code: "encrypted" }, {}],
    ];
    for (const [r, want] of cases) expect(reasonParams(r, ctx)).toEqual(want);
  });
});

describe("format", () => {
  it("formats sizes and paths", () => {
    expect(formatBytes(0)).toBe("0 B");
    expect(formatBytes(1536)).toBe("1.5 KB");
    expect(formatBytes(5 * 1024 ** 3, "tr-TR")).toBe("5,0 GB");
    expect(joinInner("\\", "a.txt")).toBe("\\a.txt");
    expect(joinInner("\\Docs", "a.txt")).toBe("\\Docs\\a.txt");
    expect(driveName("C:\\")).toBe("C:");
    expect(baseName("C:\\a\\b.txt")).toBe("b.txt");
    expect(parentPath("C:\\a\\b.txt")).toBe("C:\\a");
  });
});
