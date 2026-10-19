import type { YMD } from './dates';

export interface DisplaySeason {
  year: number;
  offSeason: boolean;
}

/** Months 4-10 show the current season; 11-3 show the most recent completed April-October season (owner decision Oct 7). */
export function displaySeason(today: YMD): DisplaySeason {
  const { year, month } = today;
  if (month >= 4 && month <= 10) return { year, offSeason: false };
  if (month >= 11) return { year, offSeason: true };
  return { year: year - 1, offSeason: true };
}
