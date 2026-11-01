# Proposal 09: "How the forecast changed" as a grid of small panels (facets)

**Status:** Accepted 2026-11-01. Decisions in section 8: High, Low and Rain stay separate tabs; **every panel shares one y axis** (decision 2, fully shared, which overrides the "same span" recommendation in sections 3 and the summary); decisions 3 to 5 as recommended. Research date: 2026-11-01. The Open-Meteo behaviour below was checked against the Previous Runs API docs and with one live call on 2026-11-01 (a generic Colorado point, not a station); the fixture numbers come from `backend/fixtures/test/open-meteo/previous_runs__helena__2026-10-17__2026-11-01.json`. Nothing here changes code yet. A static mockup (phone and desktop) accompanies this proposal.

## Plain-language summary

- Today the "How the forecast changed" chart draws eight lines (one per lead time) across the target days. Eight overlapping lines are hard to read.
- Recast: **one small panel per target date** ("Today", "Tomorrow", "Mon Nov 3", ...), like seaborn's `FacetGrid` with `col_wrap`. Inside a panel the x axis is **when the forecast was issued** (7 days before on the left, the latest on the right) and y is the forecast value. Each panel is one line (or one set of bars), so "did today's forecast drift?" is one glance at one panel.
- **No API change.** `days[]` already holds one entry per target date and `lead_days` is already the x axis of each panel. The only new logic is client-side: forecasts for future dates have "not yet issued" slots that the API fills with copies of the latest run, and the panel must not draw those (section 5).
- Grid: 2 columns on a phone, up to about 6 on desktop, balanced rows (7 panels become 4 + 3). All panels in a tab share one y axis (`sharey=True`): rain and chance of rain zero-based, temperature from the window's lowest to highest value.
- The 8 lead colours and 8 dash patterns are no longer needed and are deleted. Panels reuse the existing High, Low, Rain and Chance of rain colours.
- Delivery is three small phases; the table stays as the accessible fallback.

## Problem

Rodney: "The current plot with multiple lines on it isn't easy to interpret. There should be a plot showing how today's forecast has changed over time, then a separate one for tomorrow and so forth. This should use a faceted layout like I could get with seaborn's FacetGrid."

Today (verified in the code):

- `ForecastChangesChart.svelte`: metric tabs (Rain default, Chance of rain, High, Low), x = target day, one line per lead from `leadSeries()` (8 lines, `--c-lead-0..7`, eight widths and dashes, `line-lead-N` classes), `ChartLegend` with eight swatches, one `ChartTooltip` listing all eight leads for a target day, `role="slider"` with `aria-valuetext`, and a `changes-table`. Proposal 08 already noted that neighbouring lead colours cannot be told apart (validator row L fails) and that identity rests on width, dash, legend and tooltip: that is the symptom of the wrong chart shape, not of the palette.
- `StationForecast.svelte` mounts it in `#changes` with From/To date inputs (default today to today + 6, `FORECAST_CHANGES_DAYS = 7`; API max 14 days, `to <= today + 15`).

## 1. The recast

| | Now | Proposed |
|---|---|---|
| Unit of a chart | the whole range | one target date (a facet) |
| x | target date | when the forecast was issued: lead 7 ... lead 0 (left to right) |
| Series | 8 lines (lead) | 1 line or 1 set of bars per facet (the metric) |
| Comparison across days | by eye along x | by looking along the grid; same scale rules (section 3) |

Each panel reads: "this is what the model said about that day, starting a week out, ending with the latest run." Panel title: `Today · Sat Nov 1`, `Tomorrow · Sun Nov 2`, then `Mon Nov 3`; with an earlier `from`, `Yesterday` for -1 and plain `Fri Oct 30` otherwise. A second line gives `latest 44° · range 42-50°` (the same "moved by" idea as the table's last column).

**Data shape: confirmed, no API change.** `days[i]` is a facet; `lead_days` (0 to 7) is the x axis; `tmax_f`, `tmin_f`, `precip_in`, `prob_precip_max` are arrays indexed by lead. `leadSeries()` is replaced by a per-day `dayLeads(day, metric)` that returns the points oldest first. Lead 0 is the latest run (Open-Meteo docs: `_previous_day0` equals the live forecast), so the right-hand slot is labelled "latest", the same word the current legend uses.

## 2. Grid layout

- **Column count** is computed from the measured width (the chart already measures and draws at 1:1 pixels): `maxCols = floor((width + gap) / (MIN_CELL + gap))` with `MIN_CELL` about 150 px; then `rows = ceil(n / maxCols)` and `cols = ceil(n / rows)`, so rows are balanced. That gives 2 columns at 390 px (7 panels: 2+2+2+1), 4 at tablet and desktop for 7 panels (4+3), and for 14 panels up to 7 per row (7+7). This is `col_wrap` with sensible balancing. It lives in a pure, unit-tested `facetLayout.ts`.
- **Cell** is about 150 px tall: title and subtitle (about 34 px), plot (about 72 px), tick labels (about 22 px). Every panel shows `7d` at the left and `latest` at the right (tiny), with a tick per lead.
- **One SVG**, not 7 to 14: it keeps one `role="slider"`, one measured width, one shared tooltip layer, and no CSS grid or `style=`. Geometry is attributes (`transform` per cell); variation is classes. Same approach as `PanelStack`.
- **How many facets, and the inputs.** Default stays 7 (`FORECAST_CHANGES_DAYS`), up to the existing 14-day limit. The From/To inputs, their validation ("Pick up to 14 days.") and the request are unchanged; the grid simply has one panel per returned day.
- **Legend** (one `ChartLegend` row above the grid): "Forecast, older to latest" (line), "Level of the latest forecast" (dashed rule), "Not yet issued" (shaded). Three items instead of eight.

## 3. Shared y scale

- **Rain and chance of rain: shared, zero-based** (rain 0 to the largest value in the window, rounded; chance 0 to 100). Bars must start at a common zero, and a dry day should look dry next to a wet one. This is `sharey=True`.
- **Temperature: fully shared (decided).** One y domain for every panel in the tab, from the lowest to the highest issued value in the window (padded and `.nice()`d as today), ticks labelled on the left column only. The text below was the original recommendation and is superseded.
- *(Superseded)* **Temperature: same span, free position.** Fully shared y makes a 4 F drift on a 70 F day nearly flat when the window ranges from 25 F to 75 F; fully free y makes a 1 F wobble fill the panel and look dramatic. Recommended: every panel is the same number of degrees tall (the largest drift in the window times 1.3, at least 8 F, rounded up to an even number), but each panel is centred on its own values and labels its own ticks (at multiples of 5 F). Equal heights mean equal wobble looks equal everywhere; the tick labels keep the absolute numbers honest. The mockup uses this (26 F for the fixture week). Decision 2 lists the alternatives.

## 4. Metrics

Keep the four existing tabs (Rain default, Chance of rain, High, Low); nothing about the tab bar changes.

- **High and Low stay separate tabs.** Putting both in one panel forces a span of about 40 F (highs near 50 F, lows near 30 F), so the 3 to 8 F drift we care about shrinks to a tenth of the panel height. Separate tabs give each panel a tight scale. Each tab uses its existing colour (`--c-tmax` solid gold, `--c-tmin` blue), already contrast-tested.
- **Rain: bars.** One bar per issue day (the forecast total for that date), a dark outline on the latest bar, plus the dashed level line at the latest value. A total is an amount from zero, so bars are the honest mark; with 8 bars in a 140 px panel they are about 11 px wide, fine on a phone. A zero total is drawn as a short flat tick so "forecast zero" is visible and not confused with "no data". Colour: existing `--c-rain-bar`.
- **Chance of rain: its own tab, as a line on 0 to 100 %.** No secondary axis, and no mixing with the amount (a dual-axis facet is unreadable at 140 px). Colour: existing `--c-prob`. Probability exists only for the GFS model, and is `null` before October 2024 (documented in `02-api.md` 5.6a); the existing caption line stays.

## 5. Leads with no data, shown honestly

Two different things look like "missing" and must be drawn differently.

1. **`null` from the API** (documented: an hour is missing in that lead, or probability before October 2024). The point is not drawn; the line breaks (`linePath` already breaks on `null`) and a bar slot is left empty. Nothing in the fixture is null; this is the documented behaviour only.
2. **Not yet issued** (future target dates; verified on 2026-11-01, not in the fixture because its window ends today). Open-Meteo's offsets are tied to the valid time ("`_previous_day1` is the value that was predicted 24 hours before valid time"), but when that moment is still in the future the API does not return empty: it **returns the latest run again**. The live call showed, for the target date 4 days out, leads 0 to 3 all equal to one value and lead 4 onwards different; for dates 8 or more days out, all eight leads identical. Drawn as is, "Wed Nov 4" would show a rock-steady forecast that is really four copies of one number. The same is true of the current chart and table.
   - **Rule (pure function, unit-tested):** lead N for target date D was issued on D minus N days; it is drawn only if that date is today or earlier (station timezone), and lead 0 ("latest") always is. For a day k days ahead, leads 1 to k-1 are not issued yet. "Today" for the rule comes from `generated_at` and `timezone` (already in the response), the same value the page uses for `from`.
   - **Drawing:** the slots stay in place so every panel lines up, the not-issued span gets a light shaded background (existing `--c-night`) explained in the legend, and no point is drawn there. The latest point is joined to the rest only by position, not by a line segment across the shaded gap. A panel 7 days out has 3 points; far dates have only "latest", which is true: the API offers no earlier forecast.
   - **Table:** the same rule blanks the not-issued cells ("-") and "moved by" is computed over the issued ones only.
   - The boundary (lead N = k) mixes older and latest hours inside the one local day. Phase 1 has a builder re-check it with a live public call and adjust the rule by one if needed (the mockup draws N = k).
   - **Boundary check, 2026-11-01 (phase 1):** one call to the Previous Runs API for a generic Denver point, `temperature_2m` with `_previous_day1` to `_previous_day7`, `gfs_seamless`, local time, 2026-11-01 to 2026-11-09 (216 hours). For each target date k days ahead, leads whose hours equal lead 0 hour for hour: k=1 (Nov 2): lead 1 (24 of 24 hours). k=2 (Nov 3): leads 1 and 2. k=3: leads 1 to 3. k=4 (Nov 5): leads 1 to 4. k=5 (Nov 6): leads 1 to 5. k=6 (Nov 7): leads 1 to 6. k=7 (Nov 8) and k=8 (Nov 9): leads 1 to 7. Lead k+1 onward differs from lead 0 (k=0, today: no lead equals lead 0). So lead k is a copy too, and the rule is off by one: **lead N is issued only when N > k, i.e. D minus N is strictly before today**. The text above ("today or earlier", "leads 1 to k-1") is wrong; `leadIssued` in `web/src/lib/charts/facetLayout.ts` uses the strict rule.
   - The alternative of nulling these in `changes.rs` is possible but needs "today" there and changes the API for every future consumer; see decision 4.

## 6. Highlight, interaction, accessibility

- **Latest emphasised:** the last point gets a larger dot with its value (`44°`, `0.33 in` in the subtitle for rain), and a dashed reference line runs across the panel at the latest value, so drift is the distance between each older point and the line. The line is a class (`ref-latest`, dashed, `--c-crosshair`); within a panel it differs from the data line by dash, so colour is not the only channel.
- **Hover:** a pointer over any panel picks (panel, lead). One crosshair tick is drawn at that lead in every panel, so the reader sees "everything as of 3 days earlier" down the grid; one `ChartTooltip` box in the hovered panel shows `Tomorrow`, `3 days earlier 49°`, `latest 49° (+0°)`. It reuses `ChartTooltip` and `placeTooltip` (flip left near the right edge of the cell).
- **Keyboard:** the SVG stays one `role="slider"`. Left and Right move the lead (`useCrosshair` with `count` = 8), Up and Down (or PageUp and PageDown) move between panels, Escape clears. `aria-valuetext` reads the same tooltip text ("Tomorrow Sun Nov 2: 3 days earlier 49°, latest 49°"). This needs a small `useFacetCursor` wrapper (two indices) beside `useCrosshair`; no change to the existing hook's contract.
- **Table kept** as the accessible fallback and for exact numbers (rows = days, columns = leads, "Moved by"). It is already there and tested; it only gains the not-yet-issued blanks.
- Conventions from `01-web.md` hold: SVG only, no `style=`, 12 px labels at 1:1, classes for variation.

## 7. Colour, tokens, tests, implementation

**Tokens.** Delete `--c-lead-0..7` from the three blocks of `app.css` (light, `[data-theme='dark']`, OS-dark), the eight `.line-lead-N` rules, the `'changes leads'` entry in `PANEL_LINES` (`contrast.test.ts:218`) and the eight `c-lead-*` rows in `TOKEN_CLASS`. The test already fails on an unclassified token or an unused table entry, so the deletion is checked. Panels use `--c-tmax`, `--c-tmin`, `--c-rain-bar`, `--c-prob`, `--c-crosshair`, `--c-night`, all already classified and tested in light and dark. New classes only: `line-facet` (solid, shared), `ref-latest` (dashed), `dot-latest`, `bar-latest`, `facet-unissued`. Add a `'changes facets'` entry to `PANEL_LINES` so the dash test requires `ref-latest` to differ from the data line.

**Reuse:** `scales.ts` (`linePath`, `barPath`), `useCrosshair`, `ChartTooltip`, `tooltip.ts`, `ChartLegend`/`legend.ts`, `ChartFrame`, the measured-width `.fit` wrapper, the metric tabs, `StationForecast`'s inputs and request. **New:** `web/src/lib/charts/facetLayout.ts` (pure: `facetLayout(width, n, opts) -> { cols, cellW, cellH, cells[] }`, `facetScale` for the shared zero-based scale and the equal-span scale, and `leadIssued(date, lead, today)`), a `FacetPanel.svelte` (one cell: title, ticks, marks), and a rewrite of `ForecastChangesChart.svelte` around them. `forecast.ts`: `leadSeries` replaced by `dayLeads`; `tipRows.changeRows` replaced by a three-row `facetRows`.

**Tests that change** (from the scout list, verified): `web/tests/e2e/forecast.spec.ts` (~396-570: tooltip rows per lead, `legend-changes` swatches, high/low tab, arrows, table rows), `web/tests/e2e/csp.spec.ts:76` (hover position on the changes chart), `tipRows.test.ts` (`changeRows`), `forecast.test.ts` (`leadSeries`, `leadLabel`, `spreadAcrossLeads` kept), `contrast.test.ts:124-131` and `:218`, `fixtures.ts:216` (`forecastChanges()` gains a future-dated day so the not-issued rule is exercised without a live call). New: `facetLayout.test.ts` (columns at 390, 768, 1024 and 1400 px for 7 and 14 panels, balanced rows, scale rules, `leadIssued`), a unit test for `useFacetCursor`, an e2e hover-and-arrow test for the grid, and a column-count check at phone width.

**Phases** (Haiku builders, Sonnet reviewers, at most 3 loops per phase, as in proposal 07):

1. `facetLayout.ts`, `leadIssued`, `dayLeads`, with unit tests; the builder re-checks the lead = k boundary against the public API and the helena fixture.
2. `FacetPanel` and the rewritten chart (marks, shared scales, legend, highlight, not-issued shading), the table blanks, CSS classes, token deletion and `contrast.test.ts` updates. Visual check at 390 and 1200 px.
3. Hover, keyboard cursor, tooltip and `aria-valuetext`; e2e and CSP test updates; `01-web.md` amendments (the changes bullet at ~475 and the chart-conventions notes).

## 8. Decisions for Rodney

**Decided 2026-11-01:** (1) separate tabs for High, Low and Rain (Chance of rain keeps its tab); (2) all panels share the same y axis, for every metric; (3) to (5) as recommended; (6) **minimal text**: the plot should explain itself. Panel titles are just the day (`Today`, `Tomorrow`, `Mon 3`); no subtitle line; x ticks `7d` and `now` only; legend items one or two words (`Forecast`, `Latest`, `Not issued`); no explanatory caption beyond the required Open-Meteo attribution; tooltips terse (`3d before 49°`). This overrides the longer labels in sections 1, 2 and 6.

1. **High and Low: separate tabs or both in one panel?** Recommended: separate tabs (as now); the combined panel's 40 F span hides drift. Alternative: one panel with two lines, easier to see the day's range but small drift.
2. **Temperature y scale across panels.** Recommended: same span, each centred on its own values (equal wobble looks equal). Alternatives: fully shared (honest absolute level, drift looks tiny) or free per panel (drift is dramatic, panels not comparable). Rain and chance of rain are always shared and zero-based.
3. **Rain as bars or a line?** Recommended: bars from zero with an outlined latest bar. Alternative: a line like the temperature panels, consistent but implies a continuous quantity.
4. **Not-yet-issued forecasts.** Recommended: hide them in the client with a rule based on today's date and show a shaded gap (no API change). Alternatives: draw what the API returns (flat copies look like a steady forecast; not recommended), or null them in the API so every consumer is honest (small Rust change, needs a new "today" input and test updates).
5. **Hover and keyboard model.** Recommended: one crosshair at the same lead in every panel and one tooltip in the hovered panel; Left and Right change the lead, Up and Down change the panel. Alternative: a tooltip and crosshair confined to the hovered panel only (simpler, loses the cross-panel comparison).

Not reopened: no chart library (proposal 07 section 4), SVG-only styling and the CSP, the four metric tabs, the From/To inputs and their 14-day limit, keeping the table.
