# Replay fixtures

Everything under this directory is served by `ReplayAwn` and `ReplayAcis` (`AWN_MODE=replay`) and used by the tests.
The repository is public: no real MACs, coordinates, names, addresses or keys. `cargo test -p poller --test fixtures_lint`
enforces that (see "Lint").

**Provenance: every file here is synthetic.** The station history, the daily ACIS records and the `StnMeta` answers are
generated or hand-written; the weather is invented. Public airport coordinates in `acis/stnmeta__*.json` are
approximate and copied from the guides. Replacing them with sanitised recordings is task T7 (Rodney records one real
station with his own dev key); the layout and identifiers below do not change when that happens.

## Layout

```
awn/devices_ok.json                  key fixture-valid-key: MACs AA:BB:CC:00:01:01 and :02 (not seeded)
awn/devices_unauthorized.json        key fixture-invalid-key -> 401
awn/devices_rate_limited.json        key fixture-ratelimited-key -> 429
awn/stations/<MAC>/device.json       key fixture-poller-key: one /devices element per seeded station
awn/stations/<MAC>/history/page-NNNN.json[.gz]   newest first, <= 288 rows per page, 62 days per station
acis/<ID>__<sdate>__<edate>.json[.gz]            StnData for the seeded reference ids USW00099001..3, 1991-01-01 to 2026-09-30
acis/stnmeta__<tag>.json             StnMeta answers for the baseline picker (plus a "recorded_bbox" member, see below)
open-meteo/forecast__<tag>.json                  recorded Open-Meteo forecast (gfs_seamless), tags helena, laramie, loveland
open-meteo/ensemble_ecmwf_ifs025__<tag>.json     recorded ECMWF IFS ensemble (51 members)
open-meteo/previous_runs__<tag>__<from>__<to>.json  recorded Previous Runs window (16 days, leads 0..7)
test/<case>/...                      small synthetic scenarios for tests, same layout (root = test/<case>)
```

`<MAC>` is `AA:BB:CC:00:00:01`, `:02`, `:03` (the seeded stations in `ops/db/seed/seed.toml`). Any other API key in
replay is `Unauthorized`. Replay shifts all timestamps once at process start so the newest reading equals now.

## The synthetic stations

`:01` is generated; `:02` and `:03` are derived from it (temperatures shifted, rain counters scaled, timestamps kept)
so each keeps its own counters. The base series contains, at a fixed offset from its start, one console counter
**reset** (day 20, the total drops to zero and counts on), so the rollup exercises `rain_resets`. There are no injected
glitches or jumps: the rain rules are a high-water mark (a non-reset dip is ignored) plus reset to near zero.

Regenerate everything (deterministic):

```
cargo run -p poller --features test-support --example record_fixtures -- synth-all
```

Record real data (Rodney, T7): `... -- awn --mac <real MAC> --as AA:BB:CC:00:00:01 --days 60`,
`... -- derive --from AA:BB:CC:00:00:01 --as AA:BB:CC:00:00:02 --temp-offset-f -3.0 --rain-scale 1.15`,
`... -- acis --sid <real GHCN id> --as USW00099001`. The AWN user key is typed at a hidden prompt, never passed as an
argument or environment variable; `APP_ENV=production` is refused.

## Open-Meteo fixtures

Recorded from the live keyless APIs at the committed fake city-centre coordinates of seed stations 1 to 3
(`cargo run -p api --example record_openmeteo -- --station-index N --as <tag>`; the recorder reads the coordinates from
`ops/db/seed/seed.toml` and has no latitude/longitude flags). The recorded `latitude`/`longitude` are Open-Meteo's grid
snaps, within 0.25 degrees of those centres (the lint checks it). `ReplayOpenMeteo` picks the file nearest to the
station and shifts every time by whole days so the forecast starts on the station's current local date.

## ACIS replay rules

* `station_data(sid, sdate, edate)` slices the rows of `acis/<sid>__*` by date. A `sid` with no file replays an
  **empty result** (`Ok`, zero rows), logged once per sid at `info`; the reference sync then succeeds and writes nothing.
  This is what a baseline chosen from the `stnmeta__*` candidates (for example `USW00094035`) gets locally.
* `nearby_stations(bbox)` returns the `stnmeta__<tag>.json` whose `recorded_bbox` centre is nearest to the request's
  centre, within 1 degree; otherwise `stnmeta__empty.json`.
* **Unavailable marker:** a request whose centre is within 0.05 degrees of **latitude 0.0, longitude 0.0** fails with
  `AcisError::Unavailable`. That is how the API's "ACIS is down" tests run without a mock server.

## Lint

`backend/crates/poller/tests/fixtures_lint.rs` fails on: a 32+ character hex string; a MAC outside `AA:BB:CC:00:`;
a latitude/longitude with more than one decimal under `awn/`; an `acis/` coordinate that is not the coordinate of a
seed reference station (StnData files) or a station entry of the file itself (`stnmeta__*`, public stations only);
fewer than 12 complete years in an ACIS file; fewer than 60 days of history per MAC; a total size above 25 MB. Open-Meteo
fixtures belong to the API crate; the lint checks `open-meteo/` and `test/open-meteo/` (coordinates within 0.25
degrees of a seed city centre) when those directories exist.

`test/open-meteo/` holds a frozen copy of the helena forecast, ensemble and previous-runs recordings (taken from commit
5caf296, which has the sunrise, sunset, weather code and snow fields). The API's forecast tests read only this copy, so
re-recording the replay fixtures in `open-meteo/` never changes their dates or values. Do not re-record it. One edit:
in the ensemble file every member's temperature and precipitation are set to null from 2026-11-06T00:00 on, to keep the
tests for a series that ends with missing member values.
