import type { MdValue, YtdStation } from '$lib/api/types';
import type { Field } from './slots';

export interface Pt {
  x: number;
  y: number;
}

/** One entry per slot; missing slots are null (the path breaks; no interpolation, no zero fill). */
export function toPoints(values: MdValue[] | null, slots: string[]): (Pt | null)[] {
  if (!values) return [];
  const byMd = new Map(values.map((v) => [v.md, v.v]));
  return slots.map((md, x) => {
    const v = byMd.get(md);
    return v === undefined ? null : { x, y: v };
  });
}

/** Points after last_complete_date's md are the open (partial) day. The partial segment is joined to the last complete point. */
export function splitCurrent(current: YtdStation['current'], field: Field): { complete: MdValue[]; partial: MdValue[] } {
  const all = current[field];
  const lc = current.last_complete_date;
  if (lc === null) return { complete: [], partial: all };
  const lcMd = lc.slice(5);
  const complete = all.filter((p) => p.md <= lcMd);
  const rest = all.filter((p) => p.md > lcMd);
  const joint = complete.length > 0 && rest.length > 0 ? [complete[complete.length - 1], ...rest] : rest;
  return { complete, partial: joint };
}
