import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { describe, expect, it } from 'vitest';

// Proposal 08 section 4 ("Automated contrast check"). Reads the token blocks from app.css, so a changed colour is
// checked without editing this file. Opaque colours only: every token is a 6-digit hex. Every pair is checked in both
// themes: light (:root), dark when chosen (:root[data-theme='dark']) and dark by default (the prefers-color-scheme block).

const CSS = readFileSync(join(process.cwd(), 'src', 'app.css'), 'utf8');

/** Every rule in app.css as { selector, body }, comments stripped. Rules nested in @media are included; the
 *  @media line itself is not a rule. Quotes in selectors are normalised to single quotes. */
export function cssRules(css: string): { selector: string; body: string }[] {
  const stripped = css.replace(/\/\*[\s\S]*?\*\//g, '');
  return [...stripped.matchAll(/([^{}]+)\{([^{}]*)\}/g)].map((m) => ({ selector: m[1].trim().replace(/"/g, "'"), body: m[2] }));
}

/** The custom properties of the rule whose selector is exactly `selector`. Name is without the leading "--". */
export function tokensOf(css: string, selector: string): Map<string, string> {
  const rules = cssRules(css).filter((r) => r.selector === selector);
  if (rules.length === 0) throw new Error(`app.css has no "${selector}" rule`);
  const tokens = new Map<string, string>();
  for (const rule of rules) for (const m of rule.body.matchAll(/--([a-z0-9-]+)\s*:\s*([^;]+);/g)) tokens.set(m[1], m[2].trim());
  return tokens;
}

/** The :root custom properties (the light theme). */
export function parseRootTokens(css: string): Map<string, string> {
  return tokensOf(css, ':root');
}

export const THEME_SELECTORS = {
  light: ':root',
  dark: ":root[data-theme='dark']",
  darkByOs: ":root:not([data-theme='light'])"
} as const;

const TOKENS = parseRootTokens(CSS);
const DARK_TOKENS = tokensOf(CSS, THEME_SELECTORS.dark);
const DARK_OS_TOKENS = tokensOf(CSS, THEME_SELECTORS.darkByOs);
/** The token sets checked. The OS-driven dark block is checked as its own set (it must equal the explicit one). */
export const THEMES: Record<string, Map<string, string>> = { light: TOKENS, dark: DARK_TOKENS, darkByOs: DARK_OS_TOKENS };

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

type Rgb = [number, number, number];

function tokenRgb(tokens: Map<string, string>, n: string): Rgb {
  const v = tokens.get(n);
  if (v === undefined) throw new Error(`--${n} is not defined in its token block`);
  return hexToRgb(n, v);
}

function ratio(a: Rgb, b: Rgb): number {
  const x = luminance(a);
  const y = luminance(b);
  return (Math.max(x, y) + 0.05) / (Math.min(x, y) + 0.05);
}

/** WCAG 2.x contrast ratio between two opaque tokens (names without "--"). */
export function contrast(fgName: string, bgName: string, tokens: Map<string, string> = TOKENS): number {
  return ratio(tokenRgb(tokens, fgName), tokenRgb(tokens, bgName));
}

/** The `opacity` set on `.chart-svg .<cls>`, or null when the layer is opaque. */
export function layerOpacity(css: string, cls: string): number | null {
  let alpha: number | null = null;
  for (const rule of chartRules(css, cls)) {
    const m = /opacity:\s*([0-9.]+)/.exec(rule.body);
    if (m) alpha = Number(m[1]);
  }
  return alpha;
}

/** Chart layers drawn with a CSS opacity over the white panel. A line over them sees the composited colour. */
export const LAYER_CLASS: Record<string, string> = { 'c-winter': 'winter', 'c-cloud': 'area-cloud' };

/** The colour a line really sees: the token, composited over the panel surface when its layer has an opacity. */
export function backdropRgb(name: string, tokens: Map<string, string>, css: string = CSS): Rgb {
  const base = tokenRgb(tokens, name);
  const cls = LAYER_CLASS[name];
  const alpha = cls ? layerOpacity(css, cls) : null;
  if (alpha === null) return base;
  const under = tokenRgb(tokens, 'surface');
  return base.map((c, i) => alpha * c + (1 - alpha) * under[i]) as Rgb;
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
  'c-range-precip': 'line',
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
// A backdrop name is a token, or a layer whose opacity is composited (see backdropRgb).
export const BACKDROPS: Record<string, string[]> = {
  'c-humidity': [...DEFAULT_BACKDROPS, 'c-cloud'],
  'c-pressure': [...DEFAULT_BACKDROPS, 'c-cloud'],
  // median and daily lines drawn over the ensemble and precipitation bands
  // (cumulative rain lines are also drawn over the winter shade, composited at its opacity)
  'c-median': [...DEFAULT_BACKDROPS, ...BANDS, ...DAILY_BANDS, 'c-winter'],
  'c-prior': [...DEFAULT_BACKDROPS, ...BANDS, 'c-winter'],
  'c-current': [...DEFAULT_BACKDROPS, ...BANDS, 'c-winter'],
  'c-rain-bar': [...DEFAULT_BACKDROPS, ...BANDS],
  // the rain range outline is drawn over the precipitation bands
  'c-range-precip': [...DEFAULT_BACKDROPS, ...BANDS],
  'c-prob': [...DEFAULT_BACKDROPS, ...BANDS],
  'c-accum': [...DEFAULT_BACKDROPS, 'c-band-outer'],
  'c-tmax': [...DEFAULT_BACKDROPS, ...BANDS, ...DAILY_BANDS],
  'c-tmin': [...DEFAULT_BACKDROPS, ...BANDS, ...DAILY_BANDS],
  // the hatch lines sit on the snow fill, not on the panel
  'c-snow-outline': [...DEFAULT_BACKDROPS, 'c-snow']
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
  ['header-link', 'header'],
  // .now-label is text in the Now colour
  ['c-now', 'surface']
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
// line's stroke-dasharray must be distinct, so no two lines can share a pattern. Bars, bands, fills and the
// Now/crosshair/day lines are not listed.
export const PANEL_LINES: Record<string, string[]> = {
  'stack temperature': ['line-forecast', 'line-median', 'line-feels', 'line-dew', 'ref-freeze'],
  'stack precipitation': ['range-precip', 'line-accum', 'line-median', 'line-prob'],
  'stack sky': ['line-humidity', 'line-pressure'],
  'stack wind': ['line-wind', 'line-gust'],
  'daily temperature': ['line-tmax', 'line-tmin', 'line-median'],
  'daily precipitation': ['line-forecast', 'line-median'],
  cumulative: ['line-current', 'line-current-partial', 'line-median', 'line-prior'],
  // proposal 09 phase 2: the facet panels' data line and the latest level line (solid against dashed)
  'changes facets': ['line-facet', 'ref-latest']
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
  const cTokens = [...new Set([...TOKENS.keys(), ...DARK_TOKENS.keys(), ...DARK_OS_TOKENS.keys()])].filter((n) => n.startsWith('c-'));

  it('every --c-* token in any token block is classified', () => {
    const unclassified = cTokens.filter((n) => !(n in TOKEN_CLASS));
    expect(unclassified, 'add these to TOKEN_CLASS in contrast.test.ts').toEqual([]);
  });

  it('every TOKEN_CLASS entry names a token that exists in every theme', () => {
    const missing = Object.keys(TOKEN_CLASS).filter((n) => !Object.values(THEMES).every((t) => t.has(n)));
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

describe('dark themes mirror the light tokens', () => {
  /** The colour tokens (hex values) of a block, sorted. Font and radius tokens are not colours. */
  const colours = (t: Map<string, string>) =>
    [...t].filter(([, v]) => v.startsWith('#')).map(([k]) => k).sort();
  /** Name and value pairs, sorted, so two blocks compare exactly. */
  const pairs = (t: Map<string, string>) => [...t].sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0));

  it('the explicit dark block defines the same colour tokens as light', () => {
    expect(colours(DARK_TOKENS)).toEqual(colours(TOKENS));
  });

  it('the OS-driven dark block defines the same colour tokens as light', () => {
    expect(colours(DARK_OS_TOKENS)).toEqual(colours(TOKENS));
  });

  it('the two dark blocks have the same values (they are written out twice, so they must not drift)', () => {
    expect(pairs(DARK_OS_TOKENS)).toEqual(pairs(DARK_TOKENS));
  });
});

/** The names used as var(--x) in app.css outside the :root block and comments. */
export function referencedTokens(css: string): Set<string> {
  const stripped = css.replace(/\/\*[\s\S]*?\*\//g, '').replace(/:root\s*\{[^}]*\}/, '');
  return new Set([...stripped.matchAll(/var\(--([a-z0-9-]+)\)/g)].map((m) => m[1]));
}

describe.each(Object.entries(THEMES))('%s: chart line tokens reach 3:1 on each backdrop', (_theme, tokens) => {
  const lines = Object.entries(TOKEN_CLASS)
    .filter(([, kind]) => kind === 'line')
    .map(([name]) => name);

  it.each(lines)('%s', (token) => {
    const failures: string[] = [];
    const fg = tokenRgb(tokens, token);
    for (const backdrop of backdropsFor(token)) {
      const r = ratio(fg, backdropRgb(backdrop, tokens));
      if (r < 3) failures.push(`--${backdrop}: ${r.toFixed(2)}`);
    }
    expect(failures, `--${token} (${tokens.get(token)}) fails 3:1`).toEqual([]);
  });
});

describe.each(Object.entries(THEMES))('%s: text and UI pairs', (_theme, tokens) => {
  it.each(TEXT_PAIRS)('text --%s on --%s is at least 4.5:1', (fg, bg) => {
    const r = ratio(tokenRgb(tokens, fg), backdropRgb(bg, tokens));
    expect(r, `--${fg} on --${bg}`).toBeGreaterThanOrEqual(4.5);
  });

  it.each(UI_PAIRS)('UI --%s on --%s is at least 3:1', (fg, bg) => {
    const r = ratio(tokenRgb(tokens, fg), backdropRgb(bg, tokens));
    expect(r, `--${fg} on --${bg}`).toBeGreaterThanOrEqual(3);
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
      expect.arrayContaining(['stack temperature', 'stack precipitation', 'stack sky', 'stack wind', 'daily temperature', 'cumulative', 'changes facets'])
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

describe('token block parsing', () => {
  const css = [
    '/* :root { --fake: #000000; } */',
    ':root { --a: #111111; --font-x: serif; }',
    ":root[data-theme='dark'] { --a: #222222; }",
    '@media (prefers-color-scheme: dark) {',
    "  :root:not([data-theme=\"light\"]) { --a: #333333; }",
    '}'
  ].join('\n');

  it('reads each token block by its exact selector, quotes normalised, comments ignored', () => {
    expect(tokensOf(css, ':root').get('a')).toBe('#111111');
    expect(tokensOf(css, ":root[data-theme='dark']").get('a')).toBe('#222222');
    expect(tokensOf(css, ":root:not([data-theme='light'])").get('a')).toBe('#333333');
    expect(tokensOf(css, ':root').has('fake')).toBe(false);
  });

  it('a missing block is an error, not an empty theme', () => {
    expect(() => tokensOf(css, ':root.missing')).toThrow(/no ":root.missing" rule/);
  });

  it('the real app.css has light, explicit dark and OS dark blocks', () => {
    expect(THEMES.light.get('bg')).toBeDefined();
    expect(THEMES.dark.get('bg')).toBeDefined();
    expect(THEMES.darkByOs.get('bg')).toBeDefined();
  });
});

describe('composited layers', () => {
  it('reads the opacity of the winter shade and the cloud area from app.css', () => {
    expect(layerOpacity(CSS, 'winter')).toBe(0.7);
    expect(layerOpacity(CSS, 'area-cloud')).toBe(0.85);
    expect(layerOpacity(CSS, 'night')).toBeNull();
  });

  it('a layer with opacity is seen as the token composited over the surface, an opaque token as itself', () => {
    const tokens = new Map([['surface', '#ffffff'], ['c-winter', '#000000'], ['c-night', '#000000']]);
    const css = '.chart-svg .winter { fill: var(--c-winter); opacity: 0.5; }\n.chart-svg .night { fill: var(--c-night); }';
    expect(backdropRgb('c-winter', tokens, css)).toEqual([127.5, 127.5, 127.5]);
    expect(backdropRgb('c-night', tokens, css)).toEqual([0, 0, 0]);
  });
});
