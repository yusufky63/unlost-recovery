import { describe, expect, it } from "vitest";
import { en } from "./en";
import { tr } from "./tr";

type Tree = { [k: string]: string | Tree };

function flatten(tree: Tree, prefix = ""): Map<string, string> {
  const out = new Map<string, string>();
  for (const [k, v] of Object.entries(tree)) {
    const key = prefix ? `${prefix}.${k}` : k;
    if (typeof v === "string") out.set(key, v);
    else for (const [kk, vv] of flatten(v, key)) out.set(kk, vv);
  }
  return out;
}

const vars = (s: string) => [...s.matchAll(/\{\{(\w+)\}\}/g)].map((m) => m[1]).sort();

describe("translations", () => {
  const enMap = flatten(en as unknown as Tree);
  const trMap = flatten(tr as unknown as Tree);

  it("have the same keys", () => {
    expect([...trMap.keys()].sort()).toEqual([...enMap.keys()].sort());
  });

  it("use the same interpolation variables", () => {
    for (const [key, value] of enMap) {
      expect({ key, vars: vars(trMap.get(key) ?? "") }).toEqual({ key, vars: vars(value) });
    }
  });

  it("have no empty strings and no emoji", () => {
    const emoji = /\p{Extended_Pictographic}/u;
    for (const [key, value] of [...enMap, ...trMap]) {
      expect(value.trim().length, key).toBeGreaterThan(0);
      expect(emoji.test(value), key).toBe(false);
    }
  });
});
