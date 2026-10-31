# Proposal 08: Real legends, on-chart tooltips and a red-and-blue theme

**Status:** Accepted … implemented (uncommitted). Accepted by Rodney, Oct 25, 2026: header option B (blue-led, red accents) and one tooltip per panel (see section 6). Phases 1 to 5 are in the working tree; `docs/guides/01-web.md` describes the final state. Research date: 2026-10-25. Revised the same day after a real Adobe Color run and an adversarial review. Contrast, CVD (colour-vision deficiency) and lightness numbers were computed on 2026-10-25 with the `dataviz` skill's palette validator (`validate_palette.js`, light mode, white surface, `--pairs all`) and a small WCAG 2.x contrast script.

## Plain-language summary

- **Legends:** one shared `ChartLegend` draws a swatch that looks like the mark itself (solid, dashed, dotted line, band, bar, area, arrow) plus a label. Because the forecast stack is a single SVG, the legends are SVG too: one row in each panel's header, plus one row in the other charts. The colour-description sentences in the captions go away. The daily view's high and low lines become visibly different.
- **Tooltips:** an SVG `<g transform>` drawn last inside the same SVG, following the crosshair and flipping left near the right edge. In the stack it is **one tooltip across all four panels**. Daily view and changes chart gain hover. No `style=` attributes, so the CSP is untouched. The existing text readouts stay exactly as they are.
- **Theme:** built from the Adobe Color triad of red `#C8102E`: red and blue are the brand pair, gold is the warm accent, maroon and slate are the dark neutrals. Blue is primary (buttons, links, selected state); red is the brand accent (logo mark, active nav and selected-tab indicator, errors, the "now" line). No data series is red. Every text and UI pair passes WCAG AA and every chart line is at least 3:1 on every backdrop it is drawn over (numbers in section 3). An automated contrast test is added because the guide claims AA and nothing checks it.
- Delivery is five small phases; the theme (phase 1) and legends (phase 2) ship independently of tooltips.

## Problem

Rodney: (1) the charts need proper legends, not a text summary of what the variables are; (2) hovering a chart should show the values on the chart; (3) the app should have a nicer theme in his favourite colours, red and blue.

Today (verified in the code):

- `ForecastStack.svelte` has no legend. Colours are explained in caption sentences (~261-273). Hover works (pointer handler ~208-215, arrow keys ~217-225, `role="slider"`), but values appear in a text line under the chart (`data-testid="stack-readout"`, ~311). `PanelStack.svelte` renders **one `<svg role="slider">` containing every panel**, with one crosshair line from the first panel's top to the last panel's bottom (~95-97).
- `ForecastChart.svelte` (daily) has no legend and no hover. `tmax` and `tmin` both use `line-forecast`, so they cannot be told apart.
- `ForecastChangesChart.svelte` has a text-only legend (~74-76) and no hover; lead-day lines are one hue at decreasing opacity.
- `CumulativeChart.svelte` has an HTML swatch legend (~162-168, `.legend .swatch` in `app.css` ~610-637) and a hover readout (`data-testid="readout"`, line 169).
- Colours are CSS tokens in `app.css` lines 2-33, plus hard-coded icon hexes (~413-469). The guide (01-web.md line 608) claims WCAG AA but no test checks it.

## Constraints (already decided, not reopened)

- Hand-rolled SVG with d3-scale/d3-shape; no chart library (01-web.md line 426).
- CSP `style-src 'self'`; no `<style` or `style=` in `200.html` (`check-build.mjs` lines 35-36, `scan.test.ts`). Allowed: SVG attributes (`x`, `y`, `transform`, `width`, `height`) and CSS classes; `<defs>`/`<pattern>` elements.
- Layout from proposal 07 and the 1024 px desktop-first rule stand. Light mode only. This proposal restyles; it does not move sections.

## 1. Legends

### 1.1 Approach (one choice, stated)

**SVG legends, one row inside each panel's header, using one shared component.** `PanelStack` is a single `role="slider"` SVG, so an HTML list cannot sit between panels, and a single HTML legend above the stack would need 14+ entries far from the data. So `ChartLegend.svelte` renders a `<g class="legend" aria-hidden="true">` with, per item, a swatch (`<line>`, `<rect>` or `<path>` carrying the **chart's own series class**) and a `<text>` label. Because the swatch uses the same class as the mark, a colour or dash change in `app.css` changes chart and legend together. Item x positions come from the same character-width estimate as the tooltip (about 6.5 px per character at 12 px, no `getBBox`, so it works in unit tests). Items wrap to a second row if the row is too wide; the row count feeds the panel header height.

Props: `items: { label: string; mark: 'line' | 'band' | 'bar' | 'area' | 'arrow' | 'rule'; cls: string }[]`, `x`, `y`, `width`. An item is omitted when its series is not drawn (for example Snow when no snowfall). Dash style comes from the class in CSS, not from the item. The legend is `aria-hidden`; screen readers get the panel's `aria-label` and the live readout, as today.

Cost, which belongs to phase 2: `stackLayout.ts` gains a per-panel header height (about 20 px, 40 px if wrapped) so panel tops shift; `PanelStack` renders the legend in each panel's header; the standalone charts reserve the same header row. All other charts use the same SVG component, so the HTML `.legend .swatch` CSS is deleted.

### 1.2 Placement and items

| Chart | Placement | Items |
|---|---|---|
| Stack, hourly | Header row of each panel | Temperature: Forecast (solid), Ensemble median (long dash), Range p10-p90 (band), Feels like (short dash), Dew point (dotted), 32 F (thin grey rule). Precipitation: Hourly rain (bar), Rain range (faint bar), Snow (hatched bar), Accumulated (dashed), Accumulated range (band), Chance of precipitation (dotted). Sky: Cloud cover (area), Humidity (dashed), Pressure (dash-dot). Wind: Wind speed (solid), Gusts (dashed), Direction (arrow). |
| Stack, compact dashboard rows | One header row | Forecast, Ensemble median, Range p10-p90, 32 F |
| Daily view | Header row above plot | High (solid) and Low (dashed) with their bands, Rain median and range (bar), Chance of rain (dotted) |
| Changes chart | Header row above plot | "Forecast made N days ahead": eight small swatches, each in its real colour, width and dash, labelled `latest` ... `7 d earlier` |
| Cumulative | Header row above plot (replaces the HTML legend) | Current season (solid), Prior seasons (fine dotted grey), Median (dashed), Normal-range bands, Partial-season fill |

### 1.3 Captions

The colour sentences in `ForecastStack` (~261-273) are removed. `ProvenanceCaption` keeps source model, run time, ensemble size and units.

### 1.4 tmax / tmin and lead days

- Daily: `line-tmax` solid gold, `line-tmin` dashed blue, each with a band in a tint of its own colour. Different hue, dash and position.
- Changes chart: the eight lead lines are **opaque**, not opacity-faded (30% opacity measured about 1.7:1). The first three are solid with decreasing width; the rest use dashes. Colour is secondary: validator row L in section 3.4 shows lead 0 vs 1 are close in colour, so identity comes from width, dash, the legend and the tooltip.

## 2. Tooltips

### 2.1 Mechanism (CSP-safe)

An SVG `<g class="tip" transform="translate(x y)" aria-hidden="true">` holding a `<rect class="tip-bg">` and `<text>` rows, rendered as the **last child of the same chart SVG** so it paints on top. Only `transform`, `x`, `y`, `width`, `height` attributes and classes; no `style=`. Box size comes from row count and the character-width estimate. Colours via classes (`.tip-bg { fill: var(--surface); stroke: var(--border-strong) }`; `pointer-events: none` in CSS).

`web/src/lib/charts/tooltip.ts`: pure `placeTooltip({ x, y, w, h, plotW, plotH }) => { tx, ty }`: default 12 px to the right of the crosshair, vertically centred on the pointer; **flips to the left** when `x + 12 + w > plotW`; clamps to the SVG's top and bottom. Unit-tested.

`ChartTooltip.svelte` takes `rows: { cls?: string; label: string; value: string }[]`; a row with `cls` shows the same small swatch as the legend, so the tooltip doubles as a legend for the hovered values. Small dots (`r=3`, surface ring) mark each line's value at the crosshair.

### 2.2 Content per chart

| Chart | Tooltip |
|---|---|
| Stack | **One tooltip for the whole stack.** Header: the hour ("Tue Oct 27, 3 PM"). Grouped rows: Temperature (forecast, median with p10-p90, feels, dew); Rain (amount with p10-p90, accumulated, chance, snow if any); Sky (cloud, humidity, pressure); Wind (speed, gust, compass direction). About 14 rows, 230 px tall; it may extend over adjacent panels because every panel is in the same SVG. |
| Daily view | Date; High and Low with p10-p90; rain median and range; chance of rain; condition text. Crosshair snaps to the day. |
| Changes chart | Target date; one row per lead day with its swatch, "N d ahead: value", sorted by lead. Snaps to the target date. |
| Cumulative | The same content as its current readout, as a tooltip. |
| Dashboard compact rows | Hour, forecast, median with range. |

**One tooltip vs one per panel:** one per panel means four boxes at once, each covering its own panel. One tooltip with one crosshair matches the existing single pointer handler and the Weather Underground chart you attached. Recommended: one (decision 2).

### 2.3 Implementation in the stack

No new overlay and no extra SVG. In `PanelStack.svelte`, the hovered index already draws one crosshair line from the first panel's top to the last panel's bottom. Add `<ChartTooltip>` as the last child of that `<svg role="slider">`. Its y anchors to the pointer's y inside the SVG (arrow keys anchor to the temperature panel's mid-height). The crosshair line uses a neutral ink class (`--c-crosshair`), never red, so it cannot be confused with the red "Now" line.

### 2.4 Keyboard and accessibility

- The `role="slider"`, arrow-key handler and `aria-live` readouts stay as they are, including `data-testid="stack-readout"` and the cumulative `data-testid="readout"`. **The text readouts are not changed.** Arrow keys already move the crosshair; the tooltip follows the same index. Home/End jump to the first/last hour.
- The tooltip is `aria-hidden`; it duplicates the readout.
- Pointer leaves: tooltip and crosshair hide; keyboard focus keeps them at the focused index. Touch is out of scope.

### 2.5 Hover on the other charts

Daily view and changes chart get a pointer handler plus the slider/arrow-key/aria-live pattern, factored into `useCrosshair(xScale, indexOf)` so the charts do not each repeat about 30 lines. Cumulative moves onto the helper in the same phase, keeping its readout line.

## 3. Theme

### 3.1 How the palette was derived (Adobe Color was used)

On 2026-10-25 the colour wheel at `color.adobe.com/create/color-wheel` was run in a real browser. A plain fetch earlier returned only the page shell, which is why the first draft of this proposal wrongly said the tool had not been used; the browser run supplied the real output:

| Run | Result |
|---|---|
| Base red `#C8102E`, **Triad** | `#C8102E` red, `#1090C7` blue, `#C7BE10` gold, `#722F3A` maroon, `#293E47` slate |
| Same base, Compound | `#C8102E`, `#B610C7`, `#10C79A`, `#10C732`, `#722F3A` |
| Base blue `#1749A6`, Split complementary | `#1749A6`, `#A68A17`, `#A66617`, `#212733`, `#332F21` (a blue's complement on Adobe's RYB wheel is gold or orange, not red, which is why the palette is built from the red-base triad) |

The theme is the red-base triad. Adobe's raw colours are bright mid-tones, so several are **darkened by hand** (same hue, lower lightness) to the contrast each job needs; those derived hexes are my calculation, not Adobe output:

| Adobe colour | Contrast on white | Used as | Derived hex (contrast on white) |
|---|---|---|---|
| `#C8102E` red | 5.88 | brand red, danger, "now" line (as is) | `#C8102E` |
| `#1090C7` blue | 3.60 (too light for text) | primary blue, darkened | `#0B6EA3` (5.57); hover `#0A5F8F` (6.89); dark `#0A4F78` (8.74); data blue `#1180B5` (4.39); light data blue `#3D86B0` (4.01) |
| `#C7BE10` gold | 1.94 (far too light) | warm data accent, darkened | `#8A7A00` (4.32) |
| `#722F3A` maroon | 9.62 | dark brand text (wordmark), red hover | as is |
| `#293E47` slate | 11.20 | dark neutral: forecast line, arrows, pressure | as is; text `#1E2E36` (14.01) is a shade of it |
| Compound `#B610C7` magenta | n/a | feels-like line, darkened | `#8E1AA0` (7.49) |
| Compound `#10C732` green | n/a | humidity line, darkened | `#1E7F2B` (5.09) |

Only the triad is used for the brand and the main data colours; the two compound colours supply the third and fourth hues a four-panel chart needs.

### 3.2 Swatch preview (text)

```
UI     bg #F5F7F9   surface #FFFFFF   text #1E2E36   muted #4A5A63
       primary blue #0B6EA3   brand red #C8102E   maroon #722F3A   slate #293E47
Data   ink #293E47 | gold #8A7A00 | blue #1180B5 | violet #8E1AA0 | green #1E7F2B | navy #0A4F78
```

### 3.3 UI tokens and contrast (WCAG 2.x)

| Token | Value | Used for | Against | Ratio | Need |
|---|---|---|---|---|---|
| `--text` | `#1E2E36` | body text | `--surface #FFFFFF` / `--bg #F5F7F9` | 14.01 / 13.04 | 4.5 |
| `--muted` | `#4A5A63` | secondary text | surface / bg | 7.16 / 6.67 | 4.5 |
| `--border` | `#D3DBE0` | card and divider lines (decorative) | surface | decorative | none |
| `--border-strong` (new) | `#6B7A83` | input and button outlines | surface / bg | 4.43 / 4.13 | 3.0 |
| `--primary` (was `--accent`) | `#0B6EA3` | links, primary button | surface / bg | 5.57 / 5.18 | 4.5 |
| `--accent-text` | `#FFFFFF` | text on primary | `#0B6EA3` | 5.57 | 4.5 |
| `--primary-hover` (new) | `#0A5F8F` | hover | white text on it | 6.89 | 4.5 |
| `--primary-tint` (new) | `#E3F0F8` | selected tab and hover background | `--primary` text on it | 4.79 | 4.5 |
| `--brand` (new) | `#C8102E` | logo mark, active-nav and selected-tab indicator | surface / bg | 5.88 / 5.48 | 3.0 (UI) |
| `--brand-dark` (new) | `#722F3A` | wordmark text on light surfaces | surface | 9.62 | 4.5 |
| `--danger` | `#C8102E` | error text | surface / `--danger-bg #FBE9EC` | 5.88 / 5.03 | 4.5 |
| `--focus` (new) | `#0B6EA3` | focus ring, 2 px + 2 px offset | surface / bg | 5.57 / 5.18 | 3.0 |
| `--header` (new), option A | `#C8102E` | red header bar | white text | 5.88 | 4.5 |
| header links, option A | `#FDE3E7` | inactive nav | on `#C8102E` | 4.85 | 4.5 |
| `--header` (new), option B | `#0A4F78` | blue header bar | white text | 8.74 | 4.5 |
| header links, option B | `#D6E6F1` | inactive nav | on `#0A4F78` | 6.85 | 4.5 |
| header indicator, option B | `#FF8A9B` | active-nav underline | on `#0A4F78` | 3.90 | 3.0 (UI) |

Red on the blue header (`#C8102E` on `#0A4F78`) is 1.49, which is why option B uses a lighter red for its underline and keeps the logo mark's red inside a white ring. `--ok`, `--warn-bg` unchanged; `--info-bg` becomes `#E3F0F8`.

**Selected state** (tabs, segmented toggles): `--primary-tint` background, `--primary` text (4.79), and a 3 px `--brand` bar along the bottom edge (5.88 on white). Unselected: transparent, `--muted` text, `--border-strong` outline. The red bar is the "selected" cue, so red is part of the UI without touching data.

### 3.4 Chart tokens

Every `--c-*` token is classified `line`, `fill` or `ui`. A `line` must reach 3:1 on **each backdrop it is drawn over**. Backdrops: `--surface` white; `--c-night` `#E4E9ED` (night shading, behind every panel); `--c-cloud` `#D5DCE2` (behind humidity and pressure). Measured ratios are white / night (/ cloud where applicable).

| Token (class) | Hex | Old | White / night / cloud | Dash (distinct within its panel) |
|---|---|---|---|---|
| `--c-forecast` (`line-forecast`) | `#293E47` | `#1C2430` | 11.20 / 9.16 | solid 2 px |
| `--c-median` (`line-median`) | `#8A7A00` | `#B35C00` | 4.32 / 3.53 | `10 3` |
| `--c-feels` (`line-feels`) | `#8E1AA0` | `#7B4FA0` | 7.49 / 6.12 | `4 3` |
| `--c-dew` (`line-dew`) | `#1180B5` | `#2E7D5B` | 4.39 / 3.59 | `1 3`, round caps |
| `--c-freeze` (`ref-freeze`) | `#6B7A83` | `#5F88B8` | 4.43 / 3.63 | `6 6`, 1 px |
| `--c-rain-bar` (`bar-precip`) | `#1180B5` | `#2C6AA0` | 4.39 / 3.59 | bar shape |
| `--c-accum` (`line-accum`) | `#0A4F78` | `#1C4F7A` | 8.74 / 7.15 | `6 3` |
| `--c-prob` (`line-prob`) | `#8A7A00` | `#2F8F9D` | 4.32 / 3.53 | `1 3` |
| `--c-snow` (`bar-snow`) | fill `#8CBBD9` + 45-degree hatch, outline `#2F6F93` | `#9FB6CC` | outline 5.50 / 4.50 | hatch pattern |
| `--c-humidity` (`line-humidity`) | `#1E7F2B` | `#6AA83C` | 5.09 / 4.16 / **3.68** | `6 3` |
| `--c-pressure` (`line-pressure`) | `#293E47` | `#1C2430` | 11.20 / 9.16 / 8.09 | `8 3 1 3` |
| `--c-wind` (`line-wind`) | `#0A4F78` | `#1D5B8A` | 8.74 / 7.15 | solid 2 px |
| `--c-gust` (`line-gust`) | `#3D86B0` | `#8FB0CF` | 4.01 / **3.28** | `5 3` |
| wind arrows (`wind-arrow`) | `#293E47` | n/a | 11.20 / 9.16 | arrow shape |
| `--c-tmax` (`line-tmax`, new) | `#8A7A00` | `--c-forecast` | 4.32 / 3.53 | solid |
| `--c-tmin` (`line-tmin`, new) | `#1180B5` | `--c-forecast` | 4.39 / 3.59 | `6 3` |
| `--c-current` | `#0B6EA3` | `#0B4F8A` | 5.57 / 4.55 | solid |
| `--c-prior` | `#6B7A83` | `#7B8492` | 4.43 / 3.63 | `2 2` |
| `--c-lead-0..7` | see below, all opaque | one hue at 30-100% opacity | all >= 3.28 | see below |
| `--c-now` (`now-line`) | `#C8102E` | `#A12622` | 5.88 / 4.81 | `4 3`, labelled "Now" |
| `--c-crosshair` (new) | `#1E2E36` | n/a | 14.01 / 11.46 | solid 1 px |
| `--c-band-outer` (fill) | `#CFE3F0` | `#C7D9EA` | fill, no 3:1 | band shape |
| `--c-band-inner` (fill) | `#A9CBE2` | `#9DBDDB` | fill | band shape |
| `--c-cloud` (fill) | `#D5DCE2` | `#CFD5DC` | fill (1.38) | area shape |
| `--c-night` (fill) | `#E4E9ED` | `#E9EDF3` | fill (1.22) | n/a |
| `--c-winter` (fill) | `#E6E9EE` | same | fill | n/a |

Lead lines (changes chart): 0 `#0A3A5C` solid 2.5 px (11.85); 1 `#0A4F78` solid 2 px (8.74); 2 `#0B6EA3` solid 1.5 px (5.57); 3 `#0B6EA3` `6 3` (5.57); 4 `#3D86B0` `6 3` (4.01); 5 `#0B6EA3` `1 3` (5.57); 6 `#3D86B0` `1 3` (4.01); 7 `#6B7A83` `8 3 1 3` (4.43). Night-shade ratios for these are 9.69, 7.15, 4.55, 4.55, 3.28, 4.55, 3.28, 3.63.

Red appears in the data area in one place only: the "Now" line, which is dashed, full-height, labelled, and a different colour from the neutral crosshair. No series is red.

**Snow hatch:** the `<pattern>` id must be unique per chart instance, because several stacks can be on the dashboard at once and duplicate ids would make later stacks reference the first one's pattern. Use `id="snow-hatch-{uid}"` where `uid` comes from a module-level counter in `PanelStack.svelte`'s `<script module>`, and `fill="url(#snow-hatch-{uid})"`.

**Validator results** (light mode, white surface, `--pairs all`, run on the colours that share a panel). Neutral and dark lines (ink `#293E47`, grey `#6B7A83`, navy `#0A4F78`) fall outside the validator's lightness and chroma bands by design, so for them the guarantees are contrast numbers above plus a distinct dash within the panel.

| Panel / set | Colours validated | Result |
|---|---|---|
| Temperature | gold `#8A7A00`, violet `#8E1AA0`, blue `#1180B5` (plus ink forecast, grey freeze rule, band, which are neutral or fill) | All checks pass. Worst CVD 8.9 (violet vs blue, deutan), which is in the 6-8.x floor band, so the dash difference (`4 3` vs `1 3`) is required and present. Normal-vision worst 22.4. |
| Precipitation | rain-bar blue `#1180B5`, prob gold `#8A7A00` (plus navy accumulated, snow outline) | All pass. CVD 20.9, normal 22.4. Accumulated navy is dark (fails the lightness band by design) and relies on `6 3` dashes and being a line over bars. |
| Sky | humidity green `#1E7F2B`, pressure ink | Separation pass (CVD 25.8, normal 27.7); ink fails the chroma band by design. Distinguished by `6 3` vs `8 3 1 3`. |
| Wind | wind navy `#0A4F78`, gust `#3D86B0` (arrows ink) | Separation pass (CVD 18.2, normal 18.2); both fail the chroma/lightness bands (dark, muted blues). Gust is dashed `5 3`, wind solid. |
| Daily high/low | gold `#8A7A00`, blue `#1180B5` | All pass; CVD 20.9. |
| Changes leads (L) | the eight lead colours | **Fails** the normal-vision floor (lead 0 vs lead 1: 7.5), and CVD warns for grey vs light blue (6.9). Stated plainly: colour does not separate neighbouring leads. Width, dash, legend and tooltip do. |
| Red vs gold (R) | now-line `#C8102E` vs gold `#8A7A00` | Normal vision 22.1 and tritan 18.0 pass; **deutan 4.2 fails**: red-green deficient viewers will see them as similar. Mitigation, required rather than optional: the red line is full height, dashed `4 3` and labelled "Now"; gold data lines are never vertical; the crosshair is neutral ink, not red. |

If the "Now" line proves hard to read for deutan viewers in practice, the fallback is a primary-blue "Now" line with a white-ringed red label; that is not proposed now.

### 3.5 Token mapping, old to new

| Old | New |
|---|---|
| `--bg #f6f7f9` | `#F5F7F9` |
| `--surface` | unchanged |
| `--text #1c2430` | `#1E2E36` |
| `--muted #566070` | `#4A5A63` |
| `--border #d5dae1` | `#D3DBE0`; new `--border-strong #6B7A83` |
| `--accent #1d5b8a` | `--primary #0B6EA3` (rename in one commit) |
| `--danger #a12622` | `#C8102E` |
| `--info-bg #e8f1f8` | `#E3F0F8` |
| `--ok`, `--warn-bg` | unchanged |
| chart tokens | per 3.4; `--c-tmax`, `--c-tmin`, `--c-crosshair` new; `--c-lead-0..7` new |
| new | `--primary-hover`, `--primary-tint`, `--brand`, `--brand-dark`, `--border-strong`, `--danger-bg`, `--focus`, `--header` |
| hard-coded icon hexes (`app.css` ~413-469) | `currentColor` or tokens, so icons follow the theme (also required by the contrast test) |

### 3.6 Polish

Core (in phase 1):

- **Header:** option A or B (decision 1), same height and layout. Logo mark: a red (`--brand`) rounded square or circle with a white ring.
- **Active nav and selected tabs:** the red bar described in 3.3.
- **Links and buttons:** primary buttons `--primary` fill, hover `--primary-hover`; links `--primary`, underlined on hover and always inside running text.
- **Focus rings:** one rule, `:focus-visible { outline: 2px solid var(--focus); outline-offset: 2px }`, replacing the per-component variants (for example `.chart-svg.stack:focus-visible`); white on the header.

Optional extras (cut if you want it smaller): card shadow, day-strip warm/cool tick marks, recoloured weather icons beyond tokenising the hexes, secondary and destructive button variants.

## 4. Tests and docs

### Test touch points

- `web/tests/e2e/csp.spec.ts` line 42 uses `getByTestId('readout')` on the cumulative chart. Because the readout is unchanged this keeps passing; it is the line to re-run if the readout is ever touched. Add a hover step on the stack and the daily chart so the existing CSP-violation listener also covers the tooltip.
- `web/tests/e2e/forecast.spec.ts` (~157-170, 208) and `dashboard.spec.ts` (63, 92-95, 123, 131) do not assert the colour sentences, so removing them breaks nothing there; the review found them unaffected. Before phase 2, grep tests for the removed caption text to confirm.
- Layout: `stackLayout.ts` panel tops shift by the legend header height, so any e2e or unit assertion on panel geometry or pixel hover positions must be re-checked (the CSP spec hovers at fixed pixel offsets, for example `{ x: 200, y: 100 }`).
- `scan.test.ts` and `check-build.mjs` are unchanged; they guard that the tooltip stays style-free.
- Legend tests cannot use `getByRole('list', ...)`, because the legend is `aria-hidden` SVG inside a `role="slider"`. Test by class and test id: for example `locator('[data-testid="legend-temperature"] .line-feels')` exists, and Snow is absent when there is no snowfall.
- New unit tests: `tooltip.test.ts` (flip left near the right edge, clamp, width from text length); `ChartLegend` (swatch carries the series class; omitted items; wraps to a second row); `useCrosshair` (index from pointer x; arrows, Home/End); snow pattern ids unique across two mounted stacks.
- New e2e: hover the stack, the tooltip text contains the same values as `stack-readout`; hover the daily and changes charts; ArrowRight moves the tooltip.

### Automated contrast check

`web/src/test/contrast.test.ts` (vitest, no browser) parses the `:root` block of `app.css`:

- **Every `--c-*` token must be classified.** A `TOKEN_CLASS` table maps each token to `line`, `fill` or `ui`; the test fails on any `--c-*` token missing from the table (and on any table entry with no token), so a new colour cannot slip past.
- Each `line` token is composited over **its real backdrops** from a `BACKDROPS` map (default `--surface` and `--c-night`; `--c-humidity` and `--c-pressure` also `--c-cloud`) and must reach 3.0 on each, reporting token, backdrop and ratio on failure. Lead lines are opaque, so no alpha compositing is needed; any token with an alpha channel is rejected for `line`.
- Text pairs >= 4.5: `--text` and `--muted` on `--bg`/`--surface`; `--accent-text` on `--primary`; `--primary` on `--bg`/`--surface`/`--primary-tint`; `--danger` on `--danger-bg`; `--brand-dark` on `--surface`; header text and links on `--header` (both options, whichever ships). UI pairs >= 3.0: `--border-strong`, `--focus`, `--brand` on `--surface`/`--bg`.
- **Dash test:** a `PANEL_LINES` map lists the line classes of each panel (temperature, precipitation, sky, wind, daily, cumulative, leads). For each panel the test reads each class's `stroke-dasharray` (solid = empty) and requires all signatures within a panel to be distinct, so two lines can never both be solid or share a pattern.

### Guide sections to amend

- `docs/guides/01-web.md` 4.10 (line 608): replace the chart-token sentence (new tokens; AA enforced by `contrast.test.ts`; dash patterns are classes and must be distinct per panel). Add a short "Legends and tooltips" subsection near the chart section (line 426): `ChartLegend` (SVG, aria-hidden), `ChartTooltip`, `tooltip.ts`, `useCrosshair`, unique pattern ids, CSP rule (attributes and classes only). "No dark mode in v1" stays.
- `docs/guides/00-conventions.md`: one line that colour is never the only channel and new series need a distinct dash or shape class.
- `docs/Family Weather App Design Doc.md`: one line referencing the red/blue theme.

## 5. Phased plan

Each phase ships independently and leaves tests green.

1. **Theme and contrast test.** New `:root` tokens, header option, selected-tab red bar, links/buttons/focus rule, icon hexes to tokens, `contrast.test.ts` including the dash test. CSS plus one test; no component logic, no CSP impact.
2. **Legends.** `ChartLegend.svelte`; panel header height in `stackLayout.ts` and `PanelStack`; dash patterns in CSS; `line-tmax`/`line-tmin` split; legends in stack, compact rows, daily, changes (opaque lead ramp) and cumulative (HTML legend removed); caption sentences removed; hatch pattern with unique ids; geometry-dependent test positions re-checked. This is the largest legend step because of the layout change.
3. **Tooltip primitive and the stack.** `tooltip.ts`, `ChartTooltip.svelte`, `useCrosshair`, tooltip as last child of the stack SVG, neutral crosshair. Unit tests and the CSP hover check.
4. **Tooltips on the remaining charts.** Daily and changes charts gain hover and keyboard; cumulative and compact rows get tooltips (readouts unchanged).
5. **Docs and clean-up.** Guide amendments; delete the old `.legend`/`.swatch` CSS and unused tokens.

## 6. Open decisions for Rodney

1. **Header and how much red.** Both options keep blue primary and red as the brand accent.
   - **B (recommended default): blue-led header.** `#0A4F78` bar, white text, red logo mark in a white ring, light-red `#FF8A9B` active-nav underline (3.90). Red stays a sharp accent, so it still reads as "important" (errors, "Now").
   - **A: red-led header.** `#C8102E` bar, white text (5.88), inactive links `#FDE3E7` (4.85), active link white with a white underline. Strongest red presence, but red then covers the header, errors and the "Now" line, so red carries less meaning.
2. **Tooltip shape in the stack.** Recommended: one tooltip across all four panels. Alternative: one per panel (four smaller boxes). **Decided by Rodney, Oct 25, 2026: one tooltip per panel.** This supersedes the "one tooltip for the whole stack" text in sections 2 and the summary: each panel shows its own box with that panel's rows, all driven by the one shared crosshair and pointer handler.
