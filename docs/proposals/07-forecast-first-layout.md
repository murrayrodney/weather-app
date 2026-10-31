# Proposal 07: Forecast-first layout and a Weather Underground style chart

**Status:** Accepted by Rodney, Oct 25, 2026, with the recommended answer to all four open decisions (see section 6). Research date: 2026-10-25. The data-availability claims below were checked with live calls to the Open-Meteo APIs on 2026-10-25 (Helena rounded coordinates, no API key); nothing here changes code or the existing guides yet.

## Plain-language summary

- Opening a station shows the forecast first: a stacked set of panels on one shared time axis, like the Weather Underground 10-day chart you attached. Scrolling down gives "how the forecast changed", then the cumulative rain and GDD charts you already like, unchanged.
- The dashboard shows each selected station's temperature forecast by default, as **one row per station** (stacked, full width), with the Rain/GDD charts below the forecast rows. An overlaid "compare stations" view is a later option.
- The ensemble range (p10 to p90) is drawn as a band behind the temperature line, and behind hourly precipitation too. This was the point of the forecast page already; it moves to the top.
- Everything needed is available hourly from `gfs_seamless` and the ECMWF ensemble (verified). The deterministic part of the response grows from about 2 KB to about 30 KB (uncompressed, per station); the ensemble part is unchanged in v1. That is fine with the existing 30-minute cache.
- This replaces the current "Rainfall is the primary view" framing (Design Doc line 9): the forecast becomes the first view, with cumulative rain and GDD below it and still one scroll away.
- Charting: **keep the current hand-rolled SVG with d3-scale/d3-shape** (no new library). Synced stacked panels are straightforward; the real work is the layout, not the library.
- Delivery is five small phases; the first one (page restructure with the existing chart) is shippable without any API change.

## Problem

Rodney: the forecast should be the first thing seen for each station, and even on the dashboard the temperature forecast should be the default. Today the forecast sits behind a "Forecast" link button on the station page (`web/src/routes/(app)/stations/[id]/+page.svelte`) on a separate route, and the dashboard (`(app)/+page.svelte`) only offers Rain/GDD columns. The forecast chart shows one variable at a time (temperature or precipitation switch), so it cannot be read the way a forecast site is read, with temperature, cloud, precipitation and wind lined up on one time axis.

## Reference: what the Weather Underground chart does

A day strip (date, high/low, condition icon and text, precipitation); below it stacked panels sharing one x axis with alternating day/night shading and a vertical "now" line: (1) temperature, feels-like, dew point with a 32 F line; (2) cloud cover, chance of precipitation, chance of snow, humidity and pressure on a right axis; (3) hourly liquid precipitation with accumulated total; (4) wind speed with direction arrows. Left/right arrows page through the 10 days.

## 1. Page structure

### Station page `/stations/[id]` (recommended)

One scrolling page, top to bottom:

1. Header: station name, owner, freshness (as today).
2. **Forecast** section (new default view): day strip, then the stacked panels (section 2). A small switch `Hourly | Daily` stays; Daily (15 days) keeps the existing daily ribbons.
3. **How the forecast changed**: the existing `ForecastChangesChart` and table, moved inline under the forecast (with its date picker). It loads lazily when it scrolls into view, so opening a station costs one API call, not two.
4. **Season to date**: the existing Rain/GDD tabs, baseline panel and `CumulativeChart`, untouched.

Section anchors (`#forecast`, `#changes`, `#season`) with a slim sticky sub-nav (`Forecast | Changes | Rain and GDD`) so the old "reached by a button" problem becomes "everything is one scroll or one tap away". The `/stations/[id]/forecast` route stays as a redirect to `/stations/[id]#forecast` so bookmarks work, then is removed.

### Dashboard `/`

Constraints: the app is desktop-first with a minimum width of 1024 px (01-web.md line 591), and the dashboard lets you select up to 5 stations and today puts one column per station. Five WU-style multi-panel charts side by side do not fit; a 7-day hourly chart needs width.

Options:

| Option | What it is | Pros | Cons |
|---|---|---|---|
| A. Stacked rows | One full-width row per selected station: name, today's high/low strip, and a temperature-only forecast chart (line + ensemble band, 7 days hourly). Rain/GDD sections below, as columns as today. | Same chart code as the station page; each station gets enough width; easy to scan; up to 5 rows is a natural scroll. | Taller page; temperature only per row (full panels one tap away). |
| B. Overlaid comparison | One chart, one line per station (plus optional bands for one). | Direct comparison of stations. | Cluttered with 5 stations and bands; mixes different elevations; needs a legend; not what Rodney asked ("default for each station"). |
| C. Columns with compact charts | Keep columns, shrink each chart. | Closest to today. | At 5 columns each chart is about 250 px wide: 7 days of hourly data is unreadable. |

**Recommendation: A.** Each row is a compact `ForecastTemperature` (temperature panel only, with band, now line, 32 F line, day/night shading and a mini day strip of highs/lows and condition icons). Clicking a row title opens the station page. A toggle `Forecast | Rain | GDD` on the dashboard, with **Forecast as the default**, replaces the current Rain/GDD tabs; Rain and GDD keep the existing column layout (so the charts he likes are untouched). B can be added later as a "Compare" view if wanted, since it reuses the same data.

### Navigation

- The dashboard station selector (up to 5) stays and is the way to compare. Each row/column links to its station page.
- The station page gets the section sub-nav above; the "Forecast" link button goes away.
- Top nav is unchanged (Dashboard, Follows, Connect station, Account, Sign out). Followed stations appear in the same station selector, so reaching a station is: Dashboard, pick it, then tap its title. No new menu is proposed.

## 2. The WU-style chart

One component, `ForecastStack.svelte`, that renders one SVG per panel inside a shared container with a **shared x scale** (`d3-scale` time scale over the visible window). Panels are separate SVGs (or groups) so each has its own y axis and height; a single pointer handler on the container computes the hovered hour and every panel draws the same crosshair and value tooltip.

### Panels in v1

| Panel | Series | Band |
|---|---|---|
| Day strip | Date, high/low (daily), condition icon + text from `weather_code`, precip total (`precipitation_sum`), snow total if any | none |
| Temperature (F) | Temperature (gfs line), feels-like (apparent), dew point; 32 F reference line | ECMWF ensemble p10-p90 ribbon + p50 on temperature |
| Precipitation | Hourly liquid precipitation bars (gfs), accumulated total line (running sum from "now"), chance of precipitation as a thin line on a 0-100 % right axis | p10-p90 on hourly amount, drawn as a faint bar range; accumulated total shown with its own p10-p90 band (per-member sums, see section 3) |
| Cloud, humidity, pressure | Cloud cover area, relative humidity line, sea-level pressure (`pressure_msl`) line on a right axis (inHg) | none |
| Wind | Speed line, gusts as a lighter line, direction arrows every 3 hours (each arrow points downwind: the rotation is `wind_direction_10m` + 180 degrees, because Open-Meteo reports the direction the wind comes from) | none |

Chance of snow is not a model variable here; the precipitation panel shows hourly snowfall (`snowfall`, inches of snow, not liquid) as a separate bar series with its own labelled scale (a second right axis "Snow (in)", shown only when any snowfall is non-zero), and the day strip shows snow icons from `weather_code`. This avoids inventing a probability and avoids reading snow depth against the liquid axis.

Bands are only drawn where the ensemble provides the variable (temperature and precipitation are in v1, because they are what Rodney compares against). Wind, cloud, dew point and others are verified as available in the ECMWF ensemble (section 3), so bands for them are an option, off in v1 to keep the payload small.

### Behaviours

- **Day/night shading**: alternating bands between `sunset` and next `sunrise`, from the daily `sunrise`/`sunset` fields (local time). Cheap: a list of rects per panel.
- **Now line**: a vertical line at the current time on every panel. Cheap.
- **32 F line**: a horizontal reference line on the temperature panel, coloured to match the style of the app's freeze marker. Cheap.
- **Shared x axis, hover crosshair**: cheap with a single pointer handler; one tooltip lists the values of all panels at that hour (and the p10/p50/p90 where banded). - **Range and paging**: default 7 days hourly (the existing `FORECAST_HOURLY_DAYS`), the hourly data we request already covers 16 days. Rather than left/right buttons, use a horizontally scrollable viewport (the panels share one scroll container; the y axes stay pinned left) with 7 days visible and arrow buttons that move by 24 hours. Paging beyond the ensemble horizon (15 days; the GFS horizon is 16) just stops. The WU chart shows 10 days at once; at 7 visible days our width (about 900 px on desktop) gives about 5 px per hour, which is readable. A `7 days | 10 days | 15 days` range switch is a small extension (more squeezed, same code).
- **Hourly vs daily**: Hourly is the new WU-style stack. Daily keeps the existing daily ribbons (tmax and tmin each with p10-p90; precipitation p50 marker with range bar; probability under each day), shown above the day strip so the strip still works for 15 days. In daily mode the cloud/wind panels are hidden (they have no daily series).

### Cost summary

| Item | Cost |
|---|---|
| Temperature line, band, 32 F line, now line, day/night shading, shared axis, crosshair | Low: the current chart already does the line and ribbon and nulls; the rest is rects and a pointer handler |
| Day strip with icons | Medium: a small set of SVG icons mapped from WMO `weather_code` (about 10 categories, day/night variants for clear and partly cloudy); text labels cost nothing |
| Precipitation bars + accumulated line + band | Low to medium |
| Cloud/humidity/pressure panel with a right axis | Medium: dual axes and a legend; no new library |
| Wind arrows | Low to medium: rotated glyph markers, thinned to every 3 hours |
| Scroll/paging with pinned axes | Medium: layout work and a11y |

## 3. Data needed

### Verification (live calls, 2026-10-25, no key)

- **Forecast API, `models=gfs_seamless`, `forecast_days=16`**, Helena: every one of these hourly variables returned 384 of 384 non-null values: `temperature_2m`, `apparent_temperature`, `dew_point_2m`, `cloud_cover`, `relative_humidity_2m`, `pressure_msl` (hPa, sea level; `surface_pressure` is station pressure, about 887 hPa or 26.2 inHg at Helena, so it is not used), `wind_speed_10m`, `wind_direction_10m`, `wind_gusts_10m`, `weather_code`, `snowfall`, `precipitation`, `precipitation_probability`. Daily `sunrise`, `sunset`, `weather_code`, `temperature_2m_max/min`, `precipitation_sum`, `snowfall_sum`: 16 of 16. Units requested: `temperature_unit=fahrenheit`, `precipitation_unit=inch`, `wind_speed_unit=mph`. Pressure comes back in hPa (there is no inHg option); the API converts (1 hPa = 0.02953 inHg) so the web never converts units. `wind_speed_unit=mph` goes into the shared `common()` parameters, so it also reaches the ensemble and previous-runs calls; that is harmless (they do not request wind). Raw response about 31 KB uncompressed for all of the above.
- **Ensemble API, `models=ecmwf_ifs025`, `forecast_days=15`**: accepted all of `temperature_2m`, `precipitation`, `dew_point_2m`, `apparent_temperature`, `cloud_cover`, `relative_humidity_2m`, `surface_pressure`, `pressure_msl` (the 384/384 check for `pressure_msl` was on the deterministic call; the ensemble returned 51 members for it), `wind_speed_10m`, `wind_direction_10m`, `wind_gusts_10m`, `weather_code`, `snowfall`; each came back as 51 member series. So bands beyond temperature and precipitation are possible. Cost is linear: 12 variables for one location was about 984 KB uncompressed (about 82 KB per variable); today's two variables are about 165 KB per location. The percentiles are computed in the API, so the browser never receives member series.
- Precipitation probability remains the deterministic `gfs_seamless` value (as in proposal 06); the ensemble API does not provide it.
- Wind direction percentiles are not meaningful (circular), so direction is deterministic only if wind bands are ever added; this is a property of the variable, not a data-quality case.

### Required variables

Hourly deterministic (`gfs_seamless`, additions in bold): `temperature_2m`, `precipitation`, `precipitation_probability`, **`apparent_temperature`, `dew_point_2m`, `cloud_cover`, `relative_humidity_2m`, `pressure_msl`, `wind_speed_10m`, `wind_direction_10m`, `wind_gusts_10m`, `weather_code`, `snowfall`**.

Daily deterministic additions: **`sunrise`, `sunset`, `weather_code`, `snowfall_sum`** (existing: `temperature_2m_max/min`, `precipitation_sum`, `precipitation_probability_max`).

Ensemble (`ecmwf_ifs025`): unchanged in v1 (`temperature_2m`, `precipitation`). Optional later: `apparent_temperature`, `dew_point_2m`, `wind_speed_10m`, `cloud_cover`.

The accumulated precipitation total is computed in the API, starting at "now" (the first hour not in the past), not at local midnight. The deterministic total is the running sum of the hourly series. The ensemble total follows 02-api.md line 622 and test 7.9: build each member's running sum first, then run the existing `hourly()` percentile code in `openmeteo/percentiles.rs` over those per-member series (the same member-first approach `daily()`/`member_days()` already use). Summing hourly percentiles is not equivalent and is badly low (live check, Helena, 14 days: p50 0.08 in that way against 0.47 in from member sums; the recorded fixture gives 0.02 against 0.66), so it is not used and no caption caveat is needed.

### API response-shape changes

`GET /api/v1/stations/{id}/forecast` (02 section 5.6) gets additive fields; existing fields are unchanged so the current chart keeps working in phase 1:

Hourly data keeps the existing contract of parallel column arrays (`forecast.rs` lines 65-70, 01-web.md line 138), not row objects, so each addition is one more nullable array aligned to `time`:

- `deterministic.hourly`: add arrays `apparent_temp_f`, `dew_point_f`, `cloud_pct`, `humidity_pct`, `pressure_inhg`, `wind_mph`, `wind_dir_deg`, `gust_mph`, `weather_code`, `snow_in`, and `precip_accum_in` (running sum from now).
- `deterministic.daily`: add keys to each existing daily row object (the daily contract is row objects, not arrays) `weather_code`, `snow_in`, `sunrise`, `sunset`. Sunrise and sunset are converted in the API to UTC instants (like the other time fields, 01-web.md line 464), so the web needs no timezone parsing. The replay client (`replay.rs` lines 96-98) must shift sunrise and sunset along with the other times.
- `ensemble.hourly`: add `precip_accum_in` as a `BandSeries` (p10/p50/p90, from per-member sums as above). Existing fields unchanged.
- The existing top-level `timezone` is enough for local day boundaries and the now line; no offset field is added (one fixed offset would be wrong across the 2026-11-01 DST change).

### Cache and size

- The 30-minute cache keeps the same key per station. Entries grow from about 2 KB to about 30-40 KB (deterministic) per station; the family has a handful of stations, so memory is negligible. JSON to the browser is about 100 KB for 384 hourly rows with 13 variables before gzip, and compresses well (Caddy gzip is already on for the API).
- Upstream: the Forecast API request stays one call, but it now has 13 hourly + 8 daily variables over 16 days, about 2.4 call units by proposal 06's weighting formula (previously under 2); the ensemble call is unchanged in v1. At a 30-minute cache that remains far below the daily limit, and the per-user forecast rate limiter is unaffected.
- The dashboard needs one `/forecast` per selected station (up to 5 requests, each cached 30 minutes API-side and 2 minutes client-side). It reuses the full response; no separate reduced payload is proposed.

### Fixtures

All of `backend/fixtures/open-meteo/` (forecast, ensemble and previous-runs files for the same three location names) must be re-recorded together, in one run per station, with `backend/crates/api/examples/record_openmeteo.rs` (the forecast call carries the extra variable lists). Replay shifts each file on its own, so files recorded on different days would show different days for the same station; the stale `previous_runs__*__2026-10-03__2026-10-18.json` files are deleted, because `nearest()` breaks ties by name and would keep them. Oct 25, 2026 recording: `forecast__*` and `ensemble_ecmwf_ifs025__*` and `previous_runs__*__2026-10-11__2026-10-26`. The web fixtures are builders in `web/src/test/fixtures.ts` (lines 146-184); they are updated to the new shape. Re-recording may change values asserted in `backend/crates/api/tests/forecast.rs`, so those assertions are re-checked and updated in phase 2.

## 4. Charting approach

The web uses **hand-written SVG with `d3-scale`, `d3-shape` and `d3-array`** (no chart framework: `ChartFrame`, `CumulativeChart`, `ForecastChart`, `ForecastChangesChart`, and a small `scales.ts`). A synced stack is a layout problem rather than a library one:

- Shared x scale and one pointer handler: trivial with the existing approach.
- Mixed axes (percent right axis, pressure right axis): two `scaleLinear`s per panel; no limit.
- Bands, areas, steps, rotated glyphs: `d3-shape` `area`/`line` plus plain SVG.

Library alternatives: uPlot (very fast, supports synced cursors across charts, but we would still hand-draw bands, shading and icons and learn its plugin API); Observable Plot (concise facets but weaker interaction); ECharts (grid + multiple axes + `axisPointer` link out of the box, but about 1 MB and a different visual language, plus CSP and test churn). Nothing here needs more than SVG for 384 points x 13 series.

**Recommendation: no library change.** Extract a small `PanelStack` helper (shared scale, plot dimensions, pointer handler, day/night rects, now line) in `web/src/lib/charts`, reused by the station stack and the dashboard row. Accessibility: each panel is `role="img"` with a text summary; the crosshair state is also exposed as a live text row under the chart for keyboard users (arrow keys step by hour).

## 5. Phased plan

Each step is shippable on its own.

1. **Restructure pages (no API change).** Station page: forecast section first (existing `ForecastChart`), then changes, then Rain/GDD; sub-nav; old route redirects. Dashboard: Forecast tab default showing stacked rows per selected station. `ForecastChart` is not usable in a row as it stands: it has a fixed 520x280 viewBox (`web/src/lib/charts/scales.ts` lines 5-6, `app.css` lines 247-250), which at full width would be about 860 px tall, and its temperature/precipitation mode is internal state (`ForecastChart.svelte` lines 11-12). So this phase adds `width`, `height` and `variable` props (a compact row of about 1400x180, `variable="temperature"`), with the station page keeping its current size and switch. Test updates in this phase: `web/tests/e2e/forecast.spec.ts` (uses the `/forecast` route 12 times; now the redirect target or the station page anchor), `web/tests/e2e/csp.spec.ts` line 55 (visits `/stations/{id}/forecast`), and `web/tests/e2e/dashboard.spec.ts` (assumes Rain is the default and has no `/forecast` mock; now needs the mock and the Forecast default). Amends: `01-web.md` (views list line 24, forecast section line 463-465, routes list, task 14, Q1) and the Design Doc view list.
2. **Add data fields (API).** Extend the forecast request, parse and response shape; accumulated precipitation; unit conversion for pressure; re-record fixtures; API tests. Amends: `02-api.md` 5.6 (including line 622 and test 7.9 for per-member accumulation; 5.6a unaffected), `01-web.md` types and fixtures list, `05-local-dev` if fixture steps are listed, and the Design Doc line 9 ("Rainfall is the primary view") to say the forecast is the first view.
3. **Temperature panel + day strip + shading + now/32 F lines + crosshair (`PanelStack`).** Replaces the temperature/precipitation switch on the station page; the dashboard rows use the same panel. Amends: `01-web.md` 4.6/4.7 (chart rules, `forecast.ts` helpers `nowX`, `dayNight`, `iconFor`), tests list and `forecast.spec.ts`.
4. **Remaining panels: precipitation (bars, accumulated, band), cloud/humidity/pressure, wind with arrows; hourly paging.** Amends: `01-web.md` 4.7 and spec 7 in the test list.
5. **Polish**: range switch (7/10/15 days) and fixes found while using phases 1-4. Extra bands and a dashboard "Compare" overlay are out of scope (decisions 1 and 3). Amends: `01-web.md`.

Each phase updates the tests named in `01-web.md` section 6 (Vitest for `forecast.ts`, Playwright `forecast.spec.ts`) and keeps the "browser never calls Open-Meteo" rule and the CC BY 4.0 caption.

## 6. Decisions

Rodney, Oct 25, 2026: full-width forecast rows with a row per station; 7 days visible, paging to 15; model ranges for temperature and rain only; snowfall on its own scale plus snow icons, no snow-chance line. The options as proposed are kept below for the record.

1. **Dashboard multi-station layout.** Recommended: stacked full-width rows with temperature forecast per station (option A); Rain/GDD keep columns. Alternatives: overlaid comparison (B) as a later view.
2. **Hourly horizon on screen.** Recommended: 7 days visible with scroll/arrow paging up to 15 days (WU shows 10 at once; 10 is possible at the cost of density).
3. **Bands beyond temperature and precipitation in v1.** Recommended: no (temperature and precipitation only; the others are verified available and can follow).
4. **Chance of snow panel.** Recommended: no probability line (no variable for it); show hourly `snowfall` on its own labelled scale and snow icons from `weather_code` instead.

Not reopened: rain logic, runtime queries, Open-Meteo with no forecast storage, password login, Node 26, airports.
