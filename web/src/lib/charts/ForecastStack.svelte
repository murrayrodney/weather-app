<script lang="ts">
  import type { Forecast } from '$lib/api/types';
  import ForecastNotes from '$lib/components/ForecastNotes.svelte';
  import { FORECAST_HOURLY_DAYS, FORECAST_MAX_DAYS } from '$lib/config';
  import {
    arrowIndexes,
    arrowRotation,
    dayCells,
    dayNight,
    forecastBand,
    hourlyWindow,
    iconFor,
    lineFrom,
    midnightIndexes,
    nowX,
    pageCount,
    pageStart,
    accumDomain,
    hourlyRainDomain,
    sliceHourly,
    temperatureDomain,
    valueExtent,
    windCompass,
    type DayCell
  } from '$lib/transforms/forecast';
  import type { Pt } from '$lib/transforms/series';
  import ChartFrame from './ChartFrame.svelte';
  import DayStrip from './DayStrip.svelte';
  import PanelStack from './PanelStack.svelte';
  import type { LegendItem } from './legend';
  import { barPath, linePath, rangePath, ribbonPath } from './scales';
  import { layoutStack, STACK_RIGHT, STACK_RIGHT_AXES, type AxisSpec, type Glyph, type Layer, type PanelSpec, type StripCell } from './stackLayout';
  import { useCrosshair } from './useCrosshair.svelte';
  import { fmtRange, fmtValue as val, tipHour, type TipBox, type TipRow } from './tooltip';
  import { hourTempRows } from './tipRows';

  // The hourly WU-style stack, one time axis: a day strip, then temperature, precipitation, sky and wind panels.
  // Full mode (the station page) shows 7 days and pages one day at a time through up to 15 days; the y axes
  // are fixed to the whole range so they do not move while paging. Compact mode (a dashboard row) is the
  // temperature panel only, 7 days, no paging. The stack measures its container and draws at 1:1 pixels.
  let { forecast, compact = false }: { forecast: Forecast; compact?: boolean } = $props();

  const HOUR_MS = 3600000;
  const MAX_WIDTH = 1400;
  const FIRST_WIDTH = 960; // used until the container has been measured
  const VISIBLE = FORECAST_HOURLY_DAYS * 24;
  const tz = $derived(forecast.timezone);

  let measured = $state(0);
  const width = $derived(Math.min(MAX_WIDTH, measured || FIRST_WIDTH));
  const nowMs = Date.now();

  // Paging: `full` is every hour the API gave (up to FORECAST_MAX_DAYS); the visible window is a slice of it.
  let page = $state(0);
  const full = $derived(hourlyWindow(forecast, FORECAST_MAX_DAYS));
  const pages = $derived(compact ? 1 : pageCount(full.time.length, VISIBLE));
  const current = $derived(Math.min(page, pages - 1));
  const start = $derived(compact ? 0 : pageStart(current, full.time.length, VISIBLE));
  const hw = $derived(sliceHourly(full, start, Math.min(full.time.length, start + VISIBLE)));

  const tms = $derived(hw.time.map((t) => Date.parse(t)));
  const startMs = $derived(tms.length ? tms[0] : 0);
  const endMs = $derived(startMs + VISIBLE * HOUR_MS);
  const lastMs = $derived(tms.length ? tms[tms.length - 1] : startMs);
  const det = $derived(hw.deterministic);
  const ens = $derived(hw.ensemble);

  // Y domains: the whole range in full mode (pinned while paging), the visible window in compact mode.
  const span = $derived(compact ? hw : full);
  const tempDomain = $derived(temperatureDomain(span));
  const rainDomain = $derived(hourlyRainDomain(span));
  const accumAxis = $derived(accumDomain(span));
  const snowMax = $derived.by(() => {
    const e = span.deterministic ? valueExtent([span.deterministic.snow_in]) : null;
    return e ? e[1] : 0;
  });
  const pressureDomain = $derived.by((): [number, number] => {
    const e = span.deterministic ? valueExtent([span.deterministic.pressure_inhg]) : null;
    return e ? [e[0] - 0.05, e[1] + 0.05] : [29.8, 30.2];
  });
  const windDomain = $derived.by((): [number, number] => {
    const s: (number | null)[][] = [];
    if (span.deterministic) s.push(span.deterministic.wind_mph, span.deterministic.gust_mph);
    const hi = valueExtent(s)?.[1] ?? 0;
    return [0, Math.max(hi * 1.1, 5)];
  });

  // Legends list only the series each panel draws (same conditions as the layers below), so they cannot drift.
  const tempLegend = $derived.by((): LegendItem[] => [
    ...(det ? [{ label: 'Forecast', mark: 'line' as const, cls: 'line-forecast' }] : []),
    ...(ens
      ? [
          { label: 'Ensemble median', mark: 'line' as const, cls: 'line-median' },
          { label: 'Range p10-p90', mark: 'band' as const, cls: 'band-outer' }
        ]
      : []),
    ...(det
      ? [
          { label: 'Feels like', mark: 'line' as const, cls: 'line-feels' },
          { label: 'Dew point', mark: 'line' as const, cls: 'line-dew' }
        ]
      : []),
    { label: '32 F', mark: 'line', cls: 'ref-freeze' }
  ]);
  const precipLegend = $derived.by((): LegendItem[] => [
    ...(ens ? [{ label: 'Rain range', mark: 'line' as const, cls: 'range-precip' }] : []),
    ...(det ? [{ label: 'Hourly rain', mark: 'bar' as const, cls: 'bar-precip' }] : []),
    ...(det && snowMax > 0 ? [{ label: 'Snow', mark: 'hatch' as const, cls: 'bar-snow' }] : []),
    ...(det ? [{ label: 'Accumulated', mark: 'line' as const, cls: 'line-accum' }] : []),
    ...(ens
      ? [
          { label: 'Accumulated range', mark: 'band' as const, cls: 'band-accum' },
          { label: 'Ensemble median', mark: 'line' as const, cls: 'line-median' }
        ]
      : []),
    ...(det ? [{ label: 'Chance of precipitation', mark: 'line' as const, cls: 'line-prob' }] : [])
  ]);
  const skyLegend = $derived.by((): LegendItem[] =>
    det
      ? [
          { label: 'Cloud cover', mark: 'area', cls: 'area-cloud' },
          { label: 'Humidity', mark: 'line', cls: 'line-humidity' },
          { label: 'Pressure', mark: 'line', cls: 'line-pressure' }
        ]
      : []
  );
  const windLegend = $derived.by((): LegendItem[] =>
    det
      ? [
          { label: 'Wind speed', mark: 'line', cls: 'line-wind' },
          { label: 'Gusts', mark: 'line', cls: 'line-gust' },
          { label: 'Direction', mark: 'arrow', cls: 'wind-arrow' }
        ]
      : []
  );

  const specs = $derived.by((): PanelSpec[] => {
    const out: PanelSpec[] = [{ name: 'temperature', height: compact ? 150 : 230, domain: tempDomain, unit: 'F', legend: tempLegend }];
    if (compact) return out;
    // Precipitation panel: hourly liquid on the left; accumulated total, chance of precipitation and snow on the right.
    const precipRight: AxisSpec[] = [
      { domain: accumAxis, unit: 'Total (in)', digits: 2 },
      { domain: [0, 100], unit: '%' }
    ];
    if (snowMax > 0) precipRight.push({ domain: [0, Math.max(snowMax * 1.1, 0.25)], unit: 'Snow (in)', digits: 2 });
    out.push({ name: 'precipitation', height: 150, domain: rainDomain, unit: 'Hourly (in)', right: precipRight, legend: precipLegend });
    out.push({
      name: 'sky',
      height: 150,
      domain: [0, 100],
      unit: '%',
      right: [{ domain: pressureDomain, unit: 'inHg', digits: 2 }],
      legend: skyLegend
    });
    out.push({ name: 'wind', height: 130, domain: windDomain, unit: 'mph', legend: windLegend });
    return out;
  });

  const layout = $derived(layoutStack(width, startMs, endMs, specs, compact ? STACK_RIGHT : STACK_RIGHT_AXES));
  const xAt = (i: number) => layout.x(tms[i]);
  const barHalf = $derived(Math.max(0.5, (layout.x(startMs + HOUR_MS) - layout.x(startMs)) * 0.35));

  // Layers per panel, drawn in order. Model ranges: temperature and rain only.
  const tempLayers = $derived.by((): Layer[] => {
    const y = layout.panels[0].y;
    const out: Layer[] = [];
    if (ens) {
      const b = forecastBand(ens.temperature_f);
      out.push({ cls: 'band-outer', d: ribbonPath(b.lower, b.upper, xAt, y) });
      out.push({ cls: 'line-median', d: linePath(b.median, xAt, y) });
    }
    out.push({ cls: 'ref-freeze', d: `M${layout.left} ${y(32)}H${layout.right}` });
    if (det) {
      out.push({ cls: 'line-dew', d: linePath(lineFrom(det.dew_point_f), xAt, y) });
      out.push({ cls: 'line-feels', d: linePath(lineFrom(det.apparent_temp_f), xAt, y) });
      out.push({ cls: 'line-forecast', d: linePath(lineFrom(det.temperature_f), xAt, y) });
    }
    return out;
  });

  const precipLayers = $derived.by((): Layer[] => {
    if (compact) return [];
    const p = layout.panels[1];
    const out: Layer[] = [];
    if (ens) out.push({ cls: 'range-precip', d: rangePath(lineFrom(ens.precip_in.p10), lineFrom(ens.precip_in.p90), xAt, p.y) });
    if (det) out.push({ cls: 'bar-precip', d: barPath(det.precip_in, xAt, p.y, barHalf) });
    // Right-hand axes: [0] accumulated total, [1] chance of precipitation, [2] snow (only when there is snowfall).
    const yTotal = p.right[0].y;
    if (det && p.right[2]) out.push({ cls: 'bar-snow', d: barPath(det.snow_in, xAt, p.right[2].y, barHalf), hatch: true });
    if (ens) {
      const b = forecastBand(ens.precip_accum_in);
      out.push({ cls: 'band-accum', d: ribbonPath(b.lower, b.upper, xAt, yTotal) });
      out.push({ cls: 'line-median', d: linePath(b.median, xAt, yTotal) });
    }
    if (det) {
      out.push({ cls: 'line-accum', d: linePath(lineFrom(det.precip_accum_in), xAt, yTotal) });
      out.push({ cls: 'line-prob', d: linePath(lineFrom(det.prob_precip), xAt, p.right[1].y) });
    }
    return out;
  });

  const skyLayers = $derived.by((): Layer[] => {
    if (compact || !det) return [];
    const p = layout.panels[2];
    const zero = det.cloud_pct.map((v, i): Pt | null => (v === null ? null : { x: i, y: 0 }));
    return [
      { cls: 'area-cloud', d: ribbonPath(zero, lineFrom(det.cloud_pct), xAt, p.y) },
      { cls: 'line-humidity', d: linePath(lineFrom(det.humidity_pct), xAt, p.y) },
      { cls: 'line-pressure', d: linePath(lineFrom(det.pressure_inhg), xAt, p.right[0].y) }
    ];
  });

  const windLayers = $derived.by((): Layer[] => {
    if (compact || !det) return [];
    const y = layout.panels[3].y;
    return [
      { cls: 'line-gust', d: linePath(lineFrom(det.gust_mph), xAt, y) },
      { cls: 'line-wind', d: linePath(lineFrom(det.wind_mph), xAt, y) }
    ];
  });

  // One arrow every 3 local hours, on the speed line, turned to show where the wind blows.
  const windArrows = $derived.by((): Glyph[] => {
    if (compact || !det) return [];
    const y = layout.panels[3].y;
    const out: Glyph[] = [];
    for (const i of arrowIndexes(hw.time, tz, det.wind_dir_deg)) {
      const speed = det.wind_mph[i];
      const dir = det.wind_dir_deg[i];
      if (typeof speed === 'number' && typeof dir === 'number') out.push({ x: xAt(i), y: y(speed), rot: arrowRotation(dir) });
    }
    return out;
  });

  const daily = $derived(forecast.deterministic?.daily ?? []);
  const strip = $derived<StripCell[]>(
    dayCells(hw.time, tz).map((c: DayCell) => {
      const row = daily.find((d) => d.date === c.date);
      const x0 = layout.x(c.startMs);
      const x1 = layout.x(c.endMs);
      const icon = iconFor(row?.weather_code ?? null);
      return {
        key: c.date,
        date: formatDayTick(c.startMs, tz),
        x0,
        x1,
        cx: (x0 + x1) / 2,
        label: icon?.label ?? null,
        icon: icon?.kind ?? null,
        hi: row?.tmax_f ?? null,
        lo: row?.tmin_f ?? null,
        precip: row?.precip_in ?? null,
        snow: row?.snow_in ?? null
      };
    })
  );

  const dayLines = $derived(midnightIndexes(hw.time, tz).map((i) => ({ px: xAt(i), text: formatDayTick(tms[i], tz) })));
  const night = $derived(dayNight(daily, startMs, endMs));
  const nowPx = $derived(nowX(nowMs, startMs, endMs, layout.x));
  const pageLabel = $derived(tms.length ? `Showing ${formatDayTick(startMs, tz)} to ${formatDayTick(lastMs, tz)}` : '');

  // Hover (pointer) or arrow keys pick one hourly step; the crosshair snaps to it, the tooltips and the readout name its values.
  const xh = useCrosshair({
    count: () => hw.time.length,
    stepAt: (px) => (layout.invertX(px) - startMs) / HOUR_MS,
    xAt: (i) => xAt(i)
  });
  const hover = $derived(xh.index);
  const hoverPx = $derived(xh.px);

  /** Moves the visible window to page `p` (one day per page) and clears the crosshair, which refers to the old window. */
  function go(p: number) {
    page = Math.min(Math.max(0, p), pages - 1);
    xh.clear();
  }


  // One tooltip per panel (proposal 08 decision 2): each box lists its own panel's rows at the hovered hour, with the same
  // numbers as the readout. The compact dashboard row has only the temperature panel, so it gets only the temperature box.
  const tips = $derived.by((): TipBox[] => {
    const i = hover;
    if (i === null || !det) return [];
    const title = tipHour(tms[i], tz);
    const temp: TipRow[] = hourTempRows(det, ens, i);
    if (compact) return [{ title, rows: temp }];

    const rain: TipRow[] = [{ label: 'Hourly rain', value: val(det.precip_in[i], 2, ' in'), mark: 'bar', cls: 'bar-precip' }];
    if (ens) {
      rain.push({ label: 'Rain range', value: fmtRange(ens.precip_in.p10[i], ens.precip_in.p90[i], 2, ' in'), mark: 'line', cls: 'range-precip' });
    }
    rain.push({ label: 'Accumulated', value: val(det.precip_accum_in[i], 2, ' in'), mark: 'line', cls: 'line-accum' });
    if (ens) {
      rain.push(
        {
          label: 'Accumulated range',
          value: fmtRange(ens.precip_accum_in.p10[i], ens.precip_accum_in.p90[i], 2, ' in'),
          mark: 'band',
          cls: 'band-accum'
        },
        { label: 'Ensemble median', value: val(ens.precip_accum_in.p50[i], 2, ' in'), mark: 'line', cls: 'line-median' }
      );
    }
    rain.push({ label: 'Chance of precipitation', value: val(det.prob_precip[i], 0, '%'), mark: 'line', cls: 'line-prob' });
    if (snowMax > 0) rain.push({ label: 'Snow', value: val(det.snow_in[i], 2, ' in'), mark: 'hatch', cls: 'bar-snow' });

    const sky: TipRow[] = [
      { label: 'Cloud cover', value: val(det.cloud_pct[i], 0, '%'), mark: 'area', cls: 'area-cloud' },
      { label: 'Humidity', value: val(det.humidity_pct[i], 0, '%'), mark: 'line', cls: 'line-humidity' },
      { label: 'Pressure', value: val(det.pressure_inhg[i], 2, ' inHg'), mark: 'line', cls: 'line-pressure' }
    ];

    const dir = det.wind_dir_deg[i];
    const wind: TipRow[] = [
      { label: 'Wind speed', value: val(det.wind_mph[i], 0, ' mph'), mark: 'line', cls: 'line-wind' },
      { label: 'Gusts', value: val(det.gust_mph[i], 0, ' mph'), mark: 'line', cls: 'line-gust' },
      { label: 'Wind from', value: typeof dir === 'number' ? windCompass(dir) : 'no value' }
    ];

    return [
      { title, rows: temp },
      { title, rows: rain },
      { title, rows: sky },
      { title, rows: wind }
    ];
  });

  // The series are named by the legends in each panel's header, so the caption keeps only the model and attribution.
  const provenance = $derived([
    `Forecast: ${forecast.deterministic?.model ?? forecast.ensemble?.model ?? 'model unavailable'}${forecast.ensemble ? `; ensemble ${forecast.ensemble.model}, ${forecast.ensemble.members} members, p10-p90 range` : ''}. Times in ${tz}.`,
    forecast.attribution
  ]);

  /** Weekday and month/day of a UTC instant in the station's zone (for midnight ticks, day cells and the page label). */
  function formatDayTick(ms: number, zone: string): string {
    return new Intl.DateTimeFormat('en-US', { timeZone: zone, weekday: 'short', month: 'numeric', day: 'numeric' }).format(new Date(ms));
  }
</script>

<div class="stack-wrap" data-testid="forecast-stack" bind:clientWidth={measured}>
  <ForecastNotes {forecast} />
  <ChartFrame title={compact ? 'Temperature (F), hourly, 7 days' : 'Hourly forecast, 7 days'} {provenance}>
    {#if !compact}
      <div class="page-nav" data-testid="page-nav">
        <button type="button" disabled={current === 0} onclick={() => go(current - 1)}>Earlier day</button>
        <span class="page-range" data-testid="page-range">{pageLabel}</span>
        <button type="button" disabled={current >= pages - 1} onclick={() => go(current + 1)}>Later day</button>
      </div>
    {/if}
    <DayStrip {width} cells={strip} />
    <PanelStack
      {layout}
      layers={[tempLayers, precipLayers, skyLayers, windLayers]}
      glyphs={[[], [], [], windArrows]}
      {night}
      {dayLines}
      {nowPx}
      {hoverPx}
      index={hover}
      count={hw.time.length}
      label={compact
        ? 'Hourly temperature, feels like, dew point and ensemble range for 7 days'
        : 'Hourly temperature, precipitation, cloud, humidity, pressure and wind for 7 days'}
      {tips}
      onhover={xh.onhover}
      onkey={xh.onkey}
    />
  </ChartFrame>
</div>
