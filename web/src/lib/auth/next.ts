function hasControl(s: string): boolean {
  for (let i = 0; i < s.length; i++) {
    const c = s.charCodeAt(i);
    if (c < 0x20 || c === 0x7f) return true;
  }
  return false;
}

/** Validates a post-login destination (same origin, canonical path, no dot segments, no fragment). Returns "/" otherwise. */
export function safeNext(raw: string | null | undefined, origin: string = globalThis.location?.origin ?? 'http://localhost'): string {
  try {
    if (typeof raw !== 'string' || raw.length === 0 || raw.length > 200) return '/';
    if (!raw.startsWith('/') || raw.startsWith('//')) return '/';
    if (raw.includes('\\') || hasControl(raw)) return '/';
    const decoded = decodeURIComponent(raw);
    if (decoded.includes('\\') || hasControl(decoded)) return '/';
    const cut = raw.search(/[?#]/);
    const rawPath = cut < 0 ? raw : raw.slice(0, cut);
    const segments = rawPath.split('/').slice(1);
    for (let i = 0; i < segments.length; i++) {
      const seg = decodeURIComponent(segments[i]);
      const last = i === segments.length - 1;
      if (seg === '.' || seg === '..') return '/';
      if (seg === '' && !last) return '/';
      if (seg.includes('/') || seg.includes('\\')) return '/';
    }
    const u = new URL(raw, origin);
    if (u.origin !== origin || u.pathname !== rawPath || u.pathname.startsWith('//')) return '/';
    return u.pathname + u.search;
  } catch {
    return '/';
  }
}
