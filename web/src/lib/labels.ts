function plain(s: string): string {
  return s.replace(/_/g, ' ');
}

export const labels = {
  dayDefinition(v: string): string {
    if (v === 'local_standard_time_midnight_to_midnight') return 'Days are midnight to midnight in local standard time (no daylight-saving shift).';
    return plain(v);
  },
  baselineSource(v: string): string {
    if (v === 'ghcnd_acis') return 'NOAA GHCN-Daily via ACIS';
    return plain(v);
  },
  gddMethod(v: string): string {
    if (v === 'simple_avg') return 'Simple average';
    if (v === 'clamped_50_86') return 'Clamped to the base and 86 F';
    return plain(v);
  }
};

const MONTHS = ['January', 'February', 'March', 'April', 'May', 'June', 'July', 'August', 'September', 'October', 'November', 'December'];

/** "04-01" -> "April 1". */
export function mdLabel(md: string): string {
  const m = /^(\d{2})-(\d{2})$/.exec(md);
  if (!m) return md;
  const month = MONTHS[Number(m[1]) - 1];
  return month ? `${month} ${Number(m[2])}` : md;
}

export function monthName(n: number): string {
  return MONTHS[n - 1] ?? '';
}
