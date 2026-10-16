import { describe, expect, it } from "vitest";
import { outlook, shadowCounts } from "./outlook";
import type { Drive } from "./types";

function drive(p: Partial<Drive>): Drive {
  return {
    root: "D:\\",
    label: "",
    fileSystem: "NTFS",
    total: 100,
    free: 50,
    volume: "",
    disks: [1],
    system: false,
    removable: false,
    readable: true,
    media: "hdd",
    trim: "off",
    model: "",
    binItems: 0,
    binBytes: 0,
    ...p,
  };
}

const keys = (o: ReturnType<typeof outlook>) => o.notes.map((n) => n.key);

describe("outlook", () => {
  it("rates a quiet hard disk as good", () => {
    const o = outlook(drive({}), [0]);
    expect(o.chance).toBe("good");
    expect(o.scannable).toBe(true);
    expect(keys(o)).toEqual(["hdd"]);
  });

  it("warns about TRIM on SSDs", () => {
    const o = outlook(drive({ media: "ssd", trim: "on" }), []);
    expect(o.chance).toBe("low");
    expect(keys(o)).toContain("trimOn");
    expect(outlook(drive({ media: "ssd", trim: "unknown" }), []).chance).toBe("fair");
    expect(outlook(drive({ media: "ssd", trim: "off" }), []).chance).toBe("good");
  });

  it("caps the system drive at fair and notes the app's own disk", () => {
    const o = outlook(drive({ system: true, disks: [0] }), [0]);
    expect(o.chance).toBe("fair");
    expect(keys(o)).toEqual(["hdd", "system", "appHere"]);
  });

  it("handles removable, locked, ReFS and FAT drives", () => {
    expect(outlook(drive({ media: "usbStick", fileSystem: "FAT32", trim: "off" }), []).scannable).toBe(false);
    expect(keys(outlook(drive({ media: "sdCard", fileSystem: "exFAT" }), []))).toEqual(["removable", "fat"]);
    const locked = outlook(drive({ readable: false }), []);
    expect(locked.chance).toBe("unknown");
    expect(keys(locked)).toEqual(["locked"]);
    expect(outlook(drive({ fileSystem: "ReFS" }), []).chance).toBe("low");
  });

  it("puts Recycle Bin items first", () => {
    const o = outlook(drive({ binItems: 3 }), []);
    expect(o.notes[0]).toEqual({ key: "binHas", tone: "positive", params: { count: 3 } });
  });

  it("counts shadow copies per drive", () => {
    expect(shadowCounts(null)).toBeNull();
    expect(
      shadowCounts([
        { id: "a", device: "x", volume: "v", roots: ["C:\\"], created: 1 },
        { id: "b", device: "y", volume: "v", roots: ["c:\\"], created: 2 },
        { id: "c", device: "z", volume: "w", roots: [], created: 3 },
      ]),
    ).toEqual({ "C:\\": 2 });
  });
});
