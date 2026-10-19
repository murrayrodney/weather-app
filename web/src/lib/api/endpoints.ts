import { apiFetch } from './client';
import { ApiError } from './errors';
import { assertUuid } from './ids';
import { cached, YTD_TTL_MS, FORECAST_TTL_MS } from './cache';
import { GDD_DEFAULTS } from '$lib/config';
import type {
  BaselineCandidates,
  ConnectStatus,
  FollowRow,
  Forecast,
  ForecastChanges,
  GddSettings,
  Me,
  SaveDevice,
  SaveResponse,
  Station,
  UserHit,
  ValidateResponse,
  YtdResponse
} from './types';

const bad = () => new ApiError(502, 'bad_response', 'bad response');

function obj(v: unknown): Record<string, unknown> {
  if (typeof v !== 'object' || v === null || Array.isArray(v)) throw bad();
  return v as Record<string, unknown>;
}
function has<T>(v: unknown, key: string, check: (x: unknown) => boolean): T {
  const o = obj(v);
  if (!check(o[key])) throw bad();
  return v as T;
}
const isArr = (x: unknown) => Array.isArray(x);
const isStr = (x: unknown) => typeof x === 'string';

export function getMe(signal?: AbortSignal): Promise<Me> {
  return apiFetch<unknown>('/api/v1/me', { signal }).then((v) => has<Me>(has(v, 'id', isStr), 'display_name', isStr));
}

export async function getStations(signal?: AbortSignal): Promise<Station[]> {
  const v = await apiFetch<unknown>('/api/v1/stations', { signal });
  return has<{ stations: Station[] }>(v, 'stations', isArr).stations;
}

export function getStation(id: string, signal?: AbortSignal): Promise<Station> {
  return apiFetch<unknown>(`/api/v1/stations/${encodeURIComponent(assertUuid(id))}`, { signal }).then((v) => has<Station>(v, 'id', isStr));
}

export interface YtdParams {
  stationIds: string[];
  year: number;
  start: string;
  end: string;
  gdd?: GddSettings;
}

/** Builds the query exactly as 01 4.3 requires: comma unencoded, fixed key order, default GDD settings omitted. */
export function ytdPath(p: YtdParams): string {
  const ids = p.stationIds.map(assertUuid).join(',');
  let q = `?station_ids=${ids}&year=${encodeURIComponent(String(p.year))}&start=${encodeURIComponent(p.start)}&end=${encodeURIComponent(p.end)}`;
  const g = p.gdd;
  if (g) {
    if (g.method !== GDD_DEFAULTS.method) q += `&gdd_method=${encodeURIComponent(g.method)}`;
    if (g.base_f !== GDD_DEFAULTS.base_f) q += `&gdd_base=${encodeURIComponent(String(g.base_f))}`;
    if (g.start !== GDD_DEFAULTS.start) q += `&gdd_start=${encodeURIComponent(g.start)}`;
  }
  return '/api/v1/ytd' + q;
}

export function ytdCacheKey(p: YtdParams): string {
  const g = p.gdd ?? { method: GDD_DEFAULTS.method, base_f: GDD_DEFAULTS.base_f, start: GDD_DEFAULTS.start };
  return ['ytd', p.year, p.start, p.end, g.method, g.base_f, g.start, [...p.stationIds].sort().join(',')].join('|');
}

export function getYtd(p: YtdParams, signal?: AbortSignal): Promise<YtdResponse> {
  const path = ytdPath(p);
  return cached(ytdCacheKey(p), YTD_TTL_MS, async () => {
    const v = await apiFetch<unknown>(path, { signal });
    return has<YtdResponse>(has(v, 'year', (x) => typeof x === 'number'), 'stations', isArr);
  });
}

export async function searchUsers(q: string, signal?: AbortSignal): Promise<UserHit[]> {
  const v = await apiFetch<unknown>(`/api/v1/users/search?q=${encodeURIComponent(q)}`, { signal });
  return has<{ users: UserHit[] }>(v, 'users', isArr).users;
}

export function getBaselineCandidates(stationId: string, signal?: AbortSignal): Promise<BaselineCandidates> {
  const id = encodeURIComponent(assertUuid(stationId));
  return apiFetch<unknown>(`/api/v1/stations/${id}/baseline/candidates`, { signal }).then((v) => has<BaselineCandidates>(v, 'candidates', isArr));
}

export function putBaseline(stationId: string, referenceStationId: string): Promise<Station> {
  const id = encodeURIComponent(assertUuid(stationId));
  return apiFetch<unknown>(`/api/v1/stations/${id}/baseline`, { method: 'PUT', body: { reference_station_id: referenceStationId } }).then((v) =>
    has<Station>(v, 'id', isStr)
  );
}

export function getForecast(stationId: string, signal?: AbortSignal): Promise<Forecast> {
  const id = assertUuid(stationId);
  return cached(
    `forecast|${id}`,
    FORECAST_TTL_MS,
    async () => {
      const v = await apiFetch<unknown>(`/api/v1/stations/${encodeURIComponent(id)}/forecast`, { signal });
      return has<Forecast>(v, 'generated_at', isStr);
    },
    (f) => !f.stale
  );
}

export function getForecastChanges(stationId: string, from: string, to: string, signal?: AbortSignal): Promise<ForecastChanges> {
  const id = assertUuid(stationId);
  return cached(`forecast-changes|${id}|${from}|${to}`, FORECAST_TTL_MS, async () => {
    const v = await apiFetch<unknown>(
      `/api/v1/stations/${encodeURIComponent(id)}/forecast/changes?from=${encodeURIComponent(from)}&to=${encodeURIComponent(to)}`,
      { signal }
    );
    return has<ForecastChanges>(v, 'days', isArr);
  });
}

export async function getFollows(signal?: AbortSignal): Promise<{ following: FollowRow[]; followers: FollowRow[] }> {
  const v = await apiFetch<unknown>('/api/v1/follows', { signal });
  return has(has(v, 'following', isArr), 'followers', isArr);
}

export function requestFollow(followeeId: string): Promise<{ user: { id: string; display_name: string }; status: string }> {
  return apiFetch('/api/v1/follows', { method: 'POST', body: { followee_id: assertUuid(followeeId) } });
}

export function decideFollow(followerId: string, action: 'approve' | 'decline' | 'revoke'): Promise<FollowRow> {
  return apiFetch(`/api/v1/follows/${encodeURIComponent(assertUuid(followerId))}/${action}`, { method: 'POST' });
}

export function validateConnect(apiKey: string): Promise<ValidateResponse> {
  return apiFetch<unknown>('/api/v1/connect/validate', { method: 'POST', body: { api_key: apiKey } }).then((v) =>
    has<ValidateResponse>(v, 'valid', (x) => typeof x === 'boolean')
  );
}

export function saveConnect(apiKey: string, devices: SaveDevice[]): Promise<SaveResponse> {
  return apiFetch<unknown>('/api/v1/connect/save', { method: 'POST', body: { api_key: apiKey, devices } }).then((v) =>
    has<SaveResponse>(has(v, 'stations', isArr), 'not_covered', isArr)
  );
}

export function getConnectStatus(signal?: AbortSignal): Promise<ConnectStatus> {
  return apiFetch<unknown>('/api/v1/connect/status', { signal }).then((v) => obj(v) && (v as ConnectStatus));
}
