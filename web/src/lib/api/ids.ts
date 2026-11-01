import { ApiError } from './errors';

const UUID_RE = /^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$/;

export function isUuid(s: unknown): s is string {
  return typeof s === 'string' && UUID_RE.test(s);
}

/** Every id used in a path or in station_ids passes through here (S10). */
export function assertUuid(s: string): string {
  if (!isUuid(s)) throw new ApiError(0, 'bad_id', 'bad id');
  return s;
}
