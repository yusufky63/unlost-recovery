import type { Reason, State } from "./types";

export type Tone = "positive" | "warning" | "danger" | "neutral" | "primary";

export function stateTone(s: State): Tone {
  switch (s) {
    case "intactCopy":
    case "verified":
    case "likelyIntact":
      return "positive";
    case "partial":
      return "warning";
    case "zeroed":
    case "overwritten":
      return "danger";
    default:
      return "neutral";
  }
}

/** States shown by default: the ones worth recovering. Zeroed and overwritten files are hidden
 *  until asked for, as are name-only entries. */
export const USEFUL_STATES: State[] = ["intactCopy", "verified", "likelyIntact", "partial"];

/** Files that have nothing to copy. The engine refuses them too. */
export function recoverable(s: State): boolean {
  return s !== "metadataOnly" && s !== "notRecoverableHere";
}

/** Interpolation values for a reason's translated sentence. */
export function reasonParams(r: Reason, ctx: { ext: string; date: (unix: number) => string; bytes: (n: number) => string }): Record<string, string | number> {
  switch (r.code) {
    case "shadowCopy":
      return { date: ctx.date(r.created) };
    case "clustersFree":
      return { clusters: r.clusters, sampled: r.sampled };
    case "clustersFreeUnsampled":
      return { clusters: r.clusters };
    case "clustersReused":
      return { reused: r.reused, total: r.total };
    case "allZero":
      return { sampled: r.sampled };
    case "headerMismatch":
      return { ext: ctx.ext || "?" };
    case "unreadable":
      return { bytes: ctx.bytes(r.bytes) };
    case "someZero":
      return { zero: r.zero, total: r.total };
    default:
      return {};
  }
}
