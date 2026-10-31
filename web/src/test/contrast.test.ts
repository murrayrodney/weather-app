import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { describe, expect, it } from 'vitest';

// Proposal 08 section 4 ("Automated contrast check"). Reads the :root tokens from app.css, so a changed
// colour is checked without editing this file. Opaque colours only: every token is a 6-digit hex.

const CSS = readFileSync(join(process.cwd(), 'src', 'app.css'), 'utf8');

/** The :root custom properties, without comments. Name is without the leading "--". */
export function parseRootTokens(css: string): Map<string, string> {
  const stripped = css.replace(/\/\*[\s\S]*?\*\//g, '');
  const block = /:root\s*\{([^}]*)\}/.exec(stripped);
  if (!block) throw new Error('app.css has no :root block');
  const tokens = new Map<string, string>();
  for (const m of block[1].matchAll(/--([a-z0-9-]+)\s*:\s*([^;]+);/g)) tokens.set(m[1], m[2].trim());
  return tokens;
}

const TOKENS = parseRootTokens(CSS);

function hexToRgb(name: string, value: string): [number, number, number] {
  if (!/^#[0-9a-fA-F]{6}$/.test(value)) throw new Error(`--${name} is "${value}": only opaque #RRGGBB values are allowed`);
  return [1, 3, 5].map((i) => parseInt(value.slice(i, i + 2), 16)) as [number, number, number];
}

function luminance([r, g, b]: [number, number, number]): number {
  const lin = (c: number) => {
    const s = c / 255;
    return s <= 0.03928 ? s / 12.92 : Math.pow((s + 0.055) / 1.055, 2.4);
  };
  return 0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b);
}

/** WCAG 2.x contrast ratio between two opaque tokens (names without "--"). */
export function contrast(fgName: string, bgName: string, tokens: Map<string, string> = TOKENS): number {
  const get = (n: string) => {
    const v = tokens.get(n);
    if (v === undefined) throw new Error(`--${n} is not defined in :root`);
    return hexToRgb(n, v);
  };
  const a = luminance(get(fgName));
  const b = luminance(get(bgName));
  return (Math.max(a, b) + 0.05) / (Math.min(a, b) + 0.05);
}

// Every --c-* token must appear here. A new colour fails until it is classified.
//   line: a stroke or a mark that carries information; must reach 3:1 on each backdrop it is drawn over
//   fill: a shaded area or bar fill (no contrast floor; it is not the only channel)
export type TokenKind = 'line' | 'fill';
export const TOKEN_CLASS: Record<string, TokenKind> = {
  'c-forecast': 'line',
  'c-median': 'line',
  'c-feels': 'line',
  'c-dew': 'line',
  'c-freeze': 'line',
  'c-rain-bar': 'line',
  'c-accum': 'line',
  'c-prob': 'line',
  'c-snow': 'fill',
  'c-snow-outline': 'line',
  'c-humidity': 'line',
  'c-pressure': 'line',
  'c-wind': 'line',
  'c-gust': 'line',
  'c-arrow': 'line',
  'c-tmax': 'line',
  'c-tmin': 'line',
  'c-current': 'line',
  'c-prior': 'line',
  'c-lead-0': 'line',
  'c-lead-1': 'line',
  'c-lead-2': 'line',
  'c-lead-3': 'line',
  'c-lead-4': 'line',
  'c-lead-5': 'line',
  'c-lead-6': 'line',
  'c-lead-7': 'line',
  'c-now': 'line',
  'c-crosshair': 'line',
  'c-band-outer': 'fill',
  'c-band-inner': 'fill',
  'c-band-tmax': 'fill',
  'c-band-tmin': 'fill',
  'c-cloud': 'fill',
  'c-night': 'fill',
  'c-winter': 'fill'
};

/** The surfaces a line token is drawn over. Default: white panel and the night shade. */
const DEFAULT_BACKDROPS = ['surface', 'c-night'];
const BANDS = ['c-band-inner', 'c-band-outer'];
// The daily view's high and low bands. Its lines are drawn over both, so both count as backdrops for them.
const DAILY_BANDS = ['c-band-tmax', 'c-band-tmin'];
export const BACKDROPS: Record<string, string[]> = {
  'c-humidity': [...DEFAULT_BACKDROPS, 'c-cloud'],
  'c-pressure': [...DEFAULT_BACKDROPS, 'c-cloud'],
  // median and daily lines drawn over the ensemble and precipitation bands
  'c-median': [...DEFAULT_BACKDROPS, ...BANDS, ...DAILY_BANDS],
  'c-prior': [...DEFAULT_BACKDROPS, ...BANDS],
  'c-current': [...DEFAULT_BACKDROPS, ...BANDS],
  'c-rain-bar': [...DEFAULT_BACKDROPS, ...BANDS],
  'c-prob': [...DEFAULT_BACKDROPS, ...BANDS],
  'c-accum': [...DEFAULT_BACKDROPS, 'c-band-outer'],
  'c-tmax': [...DEFAULT_BACKDROPS, ...BANDS, ...DAILY_BANDS],
  'c-tmin': [...DEFAULT_BACKDROPS, ...BANDS, ...DAILY_BANDS]
};
export const backdropsFor = (token: string) => BACKDROPS[token] ?? DEFAULT_BACKDROPS;

/** Text pairs (foreground, background) and their floor: 4.5:1 (WCAG AA normal text). */
export const TEXT_PAIRS: [string, string][] = [
  ['text', 'bg'],
  ['text', 'surface'],
  ['text', 'info-bg'],
  ['text', 'warn-bg'],
  ['muted', 'bg'],
  ['muted', 'surface'],
  ['accent-text', 'primary'],
  ['accent-text', 'primary-hover'],
  ['primary', 'bg'],
  ['primary', 'surface'],
  ['primary', 'primary-tint'],
  ['danger', 'surface'],
  ['danger', 'danger-bg'],
  ['ok', 'surface'],
  ['warn', 'surface'],
  ['warn', 'warn-bg'],
  ['day-snow', 'day-cell'],
  ['muted', 'c-night'],
  ['muted', 'day-cell'],
  ['header-text', 'header'],
  ['header-link', 'header']
];

/** UI pairs (borders, focus rings, indicators): 3:1 (WCAG 1.4.11). */
export const UI_PAIRS: [string, string][] = [
  ['border-strong', 'surface'],
  ['border-strong', 'bg'],
  ['focus', 'surface'],
  ['focus', 'bg'],
  ['brand', 'surface'],
  ['brand', 'bg'],
  ['header-indicator', 'header']
];

// Dash patterns (proposal 08 section 4). Each panel lists the line classes it draws; within a panel every
// line's stroke-dasharray must be distinct, so no two lines can share a pattern. Leads are one panel because the
// changes chart draws all eight in one plot. Bars, bands, fills and the Now/crosshair/day lines are not listed.
export const PANEL_LINES: Record<string, string[]> = {
  'stack temperature': ['line-forecast', 'line-median', 'line-feels', 'line-dew', 'ref-freeze'],
  'stack precipitation': ['range-precip', 'line-accum', 'line-median', 'line-prob'],
  'stack sky': ['line-humidity', 'line-pressure'],
  'stack wind': ['line-wind', 'line-gust'],
  'daily temperature': ['line-tmax', 'line-tmin', 'line-median'],
  'daily precipitation': ['line-forecast', 'line-median'],
  cumulative: ['line-current', 'line-current-partial', 'line-median', 'line-prior'],
  'changes leads': ['line-lead-0', 'line-lead-1', 'line-lead-2', 'line-lead-3', 'line-lead-4', 'line-lead-5', 'line-lead-6', 'line-lead-7']
};

/**
 * Every rule whose selector list names `.chart-svg .<cls>`, grouped selectors included (the lead lines share one
 * `fill: none` block with their own rules). Comments are stripped first.
 */
function chartRules(css: string, cls: string): { selectors: string[]; body: string }[] {
  const target = `.chart-svg .${cls}`;
  const out: { selectors: string[]; body: string }[] = [];
  for (const m of css.replace(/\/\*[\s\S]*?\*\//g, '').matchAll(/([^{}]+)\{([^}]*)\}/g)) {
    const selectors = m[1].split(',').map((s) => s.trim());
    if (selectors.includes(target)) out.push({ selectors, body: m[2] });
  }
  return out;
}

/**
 * The stroke-dasharray of `.chart-svg .<cls>`, normalised; "solid" when no rule sets one. As in the cascade, the
 * last rule that sets a dash wins. A final declaration without a trailing ";" is still read.
 */
export function dashSignature(css: string, cls: string): string {
  let sig = 'solid';
  for (const rule of chartRules(css, cls)) {
    const dash = /stroke-dasharray:\s*([^;}]+)/.exec(rule.body);
    if (dash && dash[1].trim() !== 'none') sig = dash[1].trim().split(/\s+/).join(' ');
  }
  return sig;
}

/** True when app.css has a `.chart-svg .<cls>` rule. Without one, dashSignature would read "solid" by default. */
export function hasChartRule(css: string, cls: string): boolean {
  return chartRules(css, cls).length > 0;
}

describe('chart token classes', () => {
  const cTokens = [...TOKENS.keys()].filter((n) => n.startsWith('c-'));

  it('every --c-* token in :root is classified', () => {
    const unclassified = cTokens.filter((n) => !(n in TOKEN_CLASS));
    expect(unclassified, 'add these to TOKEN_CLASS in contrast.test.ts').toEqual([]);
  });

  it('every TOKEN_CLASS entry names a token that exists in :root', () => {
    const missing = Object.keys(TOKEN_CLASS).filter((n) => !TOKENS.has(n));
    expect(missing).toEqual([]);
  });

  // A line token that no rule uses is not tested in practice: it would pass the numbers and never draw.
  const used = referencedTokens(CSS);

  it('every line token is referenced as var(--c-x) in app.css', () => {
    const lines = Object.entries(TOKEN_CLASS)
      .filter(([, kind]) => kind === 'line')
      .map(([name]) => name);
    const unused = lines.filter((n) => !used.has(n));
    expect(unused, 'these line tokens are not used in app.css, so they would pass the contrast numbers without drawing').toEqual([]);
  });
});

/** The names used as var(--x) in app.css outside the :root block and comments. */
export function referencedTokens(css: string): Set<string> {
  const stripped = css.replace(/\/\*[\s\S]*?\*\//g, '').replace(/:root\s*\{[^}]*\}/, '');
  return new Set([...stripped.matchAll(/var\(--([a-z0-9-]+)\)/g)].map((m) => m[1]));
}

describe('chart line tokens reach 3:1 on each backdrop', () => {
  const lines = Object.entries(TOKEN_CLASS)
    .filter(([, kind]) => kind === 'line')
    .map(([name]) => name);

  it.each(lines)('%s', (token) => {
    const failures: string[] = [];
    for (const backdrop of backdropsFor(token)) {
      const ratio = contrast(token, backdrop);
      if (ratio < 3) failures.push(`--${backdrop}: ${ratio.toFixed(2)}`);
    }
    expect(failures, `--${token} (${TOKENS.get(token)}) fails 3:1`).toEqual([]);
  });
});

describe('text and UI pairs', () => {
  it.each(TEXT_PAIRS)('text --%s on --%s is at least 4.5:1', (fg, bg) => {
    const ratio = contrast(fg, bg);
    expect(ratio, `--${fg} on --${bg}`).toBeGreaterThanOrEqual(4.5);
  });

  it.each(UI_PAIRS)('UI --%s on --%s is at least 3:1', (fg, bg) => {
    const ratio = contrast(fg, bg);
    expect(ratio, `--${fg} on --${bg}`).toBeGreaterThanOrEqual(3);
  });
});

describe('dash patterns', () => {
  it('reads the stroke-dasharray of a class (solid when none)', () => {
    const css = '.chart-svg .a { fill: none; stroke-dasharray: 6 3; }\n.chart-svg .b { stroke-width: 2; }';
    expect(dashSignature(css, 'a')).toBe('6 3');
    expect(dashSignature(css, 'b')).toBe('solid');
    expect(dashSignature(css, 'missing')).toBe('solid');
  });

  it('the last rule that sets a dash wins, and a final declaration without ";" is read', () => {
    const css = '.chart-svg .a { stroke-dasharray: 6 3; }\n.chart-svg .a { stroke-dasharray: 2 2 }';
    expect(dashSignature(css, 'a')).toBe('2 2');
    const later = '.chart-svg .b { stroke-dasharray: 6 3; }\n.chart-svg .b { stroke-width: 2; }';
    expect(dashSignature(later, 'b')).toBe('6 3');
  });

  it('reads a class that is also named in a grouped selector list', () => {
    const css = '.chart-svg .a,\n.chart-svg .b {\n fill: none;\n}\n.chart-svg .b {\n stroke-dasharray: 2 2;\n}';
    expect(dashSignature(css, 'b')).toBe('2 2');
    expect(dashSignature(css, 'a')).toBe('solid');
    expect(hasChartRule(css, 'a')).toBe(true);
    expect(hasChartRule(css, 'missing')).toBe(false);
  });

  it('PANEL_LINES covers every chart panel', () => {
    expect(Object.keys(PANEL_LINES)).toEqual(
      expect.arrayContaining(['stack temperature', 'stack precipitation', 'stack sky', 'stack wind', 'daily temperature', 'cumulative', 'changes leads'])
    );
  });

  it.each(Object.entries(PANEL_LINES))('panel %s has a distinct dash for every line', (_panel, classes) => {
    const sigs = classes.map((c) => dashSignature(CSS, c));
    expect(new Set(sigs).size, sigs.join(' | ')).toBe(sigs.length);
  });

  it.each(Object.entries(PANEL_LINES))('panel %s lines each have their own chart rule in app.css', (_panel, classes) => {
    expect(classes.filter((c) => !hasChartRule(CSS, c))).toEqual([]);
  });

  it('a panel with two solid lines fails the distinct-dash rule', () => {
    const css = '.chart-svg .a { stroke: red; }\n.chart-svg .b { stroke: blue; }';
    const sigs = ['a', 'b'].map((c) => dashSignature(css, c));
    expect(new Set(sigs).size).toBe(1);
  });
});

describe('contrast helper', () => {
  it('matches the WCAG value for black on white', () => {
    expect(contrast('text', 'surface', new Map([['text', '#000000'], ['surface', '#ffffff']]))).toBeCloseTo(21, 5);
  });

  it('rejects a token with an alpha channel', () => {
    expect(() => contrast('x', 'y', new Map([['x', '#00000080'], ['y', '#ffffff']]))).toThrow(/opaque/);
  });
});
