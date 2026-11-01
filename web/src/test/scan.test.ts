import { readdirSync, readFileSync, statSync } from 'node:fs';
import { join } from 'node:path';
import { describe, expect, it } from 'vitest';

const SRC = join(process.cwd(), 'src');

function* walk(dir: string): Generator<string> {
  for (const n of readdirSync(dir)) {
    const p = join(dir, n);
    if (statSync(p).isDirectory()) yield* walk(p);
    else yield p;
  }
}

const strip = (s: string) => s.replace(/<style[\s\S]*?<\/style>/g, '');

describe('source scan (CSP-hostile constructs)', () => {
  const svelte = [...walk(SRC)].filter((f) => f.endsWith('.svelte'));
  const html = readFileSync(join(SRC, 'app.html'), 'utf8');
  const banned = ['style=', 'style:', '{@html', 'innerHTML', 'eval(', 'new Function', 'transition:', 'animate:'];

  it('has files to scan', () => {
    expect(svelte.length).toBeGreaterThan(10);
  });
  it.each(banned)('no %s in .svelte or app.html', (needle) => {
    for (const f of svelte) expect(strip(readFileSync(f, 'utf8')), f).not.toContain(needle);
    expect(html).not.toContain(needle);
  });
  it('app.html has no <style tag', () => {
    expect(html).not.toMatch(/<style/i);
  });
  it('no percentile computation in the web (the API computes them)', () => {
    for (const f of walk(SRC)) {
      if (!/\.(ts|svelte)$/.test(f) || f.endsWith('.test.ts') || f.endsWith('scan.test.ts')) continue;
      expect(readFileSync(f, 'utf8'), f).not.toMatch(/quantile\(|d3\.quantile|percentile\s*\(/i);
    }
  });
  it('set-password page strips the fragment before its first await', () => {
    const src = readFileSync(join(SRC, 'routes', '(public)', 'set-password', '+page.svelte'), 'utf8');
    const onMount = src.slice(src.indexOf('onMount('), src.indexOf('async function submit'));
    expect(onMount.indexOf('takeTokenFromLocation()')).toBeGreaterThan(-1);
    expect(onMount.indexOf('await')).toBe(-1); // nothing in the mount handler awaits at all
  });
  it('external links carry rel="noopener noreferrer"', () => {
    for (const f of svelte) {
      const src = readFileSync(f, 'utf8');
      for (const m of src.matchAll(/<a\b[^>]*target="_blank"[^>]*>/g)) expect(m[0]).toContain('rel="noopener noreferrer"');
    }
  });
});
