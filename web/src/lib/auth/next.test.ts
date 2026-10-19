import { describe, expect, it } from 'vitest';
import { safeNext } from './next';

const O = 'http://localhost:8000';

describe('safeNext', () => {
  it('accepts normal paths', () => {
    expect(safeNext('/stations/abc?x=1', O)).toBe('/stations/abc?x=1');
    expect(safeNext('/', O)).toBe('/');
    expect(safeNext('/follows', O)).toBe('/follows');
  });
  it('never returns a fragment', () => {
    expect(safeNext('/follows#frag', O)).toBe('/follows');
  });
  it.each([
    '//evil.com',
    '/\\evil',
    '/\t/evil.com',
    '/%5C%5Cevil',
    '/%5Cevil.com',
    '/.//evil.com',
    '/a/..//evil.com',
    '/%2e//evil.com',
    '/a/../b',
    'https://evil.com',
    'javascript:alert(1)',
    '/a\\b',
    '/a\u0001b',
    '',
    null,
    '/' + 'a'.repeat(200)
  ])('rejects %j', (raw) => {
    expect(safeNext(raw as string | null, O)).toBe('/');
  });
});
