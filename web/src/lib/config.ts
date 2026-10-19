export const DEFAULT_SEASON = { start: '04-01', end: '10-31' } as const; // F6
export const FULL_YEAR = { start: '01-01', end: '12-31' } as const;
export const GDD_DEFAULTS = { method: 'simple_avg', base_f: 50, start: '04-01' } as const; // owner decision Oct 7; mirrors the API defaults
export const GDD_BASE_RANGE = { min: 32, max: 70 } as const;
export const WINTER_MONTHS = [11, 12, 1, 2, 3] as const;
export const YTD_MAX_STATIONS = 5;
export const STALE_AFTER_MIN = 30; // Q9
export const FORECAST_CHANGES_DAYS = 7; // default window of the change view, from today (API max span 14)
export const FORECAST_HOURLY_DAYS = 7; // hourly chart horizon (the API returns up to 15 or 16 days)
export const FEATURES = { forecast: true } as const; // design doc: v1 requirement (forecast chart and change view)
// Fallback zone picker list (content Q8): a short list of US zones; the API normally derives the zone from AWN.
export const TIMEZONE_CHOICES: string[] = [
  'America/New_York',
  'America/Chicago',
  'America/Denver',
  'America/Phoenix',
  'America/Los_Angeles',
  'America/Anchorage',
  'Pacific/Honolulu'
];
