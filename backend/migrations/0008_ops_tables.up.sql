-- 0008 ops_tables (04 section 5.9)
CREATE TABLE app.job_runs (
  id          uuid PRIMARY KEY DEFAULT gen_random_uuid(),     -- uuid: conventions say IDs are UUIDs; no sequence grants
  job         text NOT NULL CHECK (job ~ '^[a-z][a-z_]*$'),   -- awn_ingest, awn_backfill, daily_rollup, reference_sync, credential_reencrypt
  subject_id  uuid,                                            -- optional station / credential id
  started_at  timestamptz NOT NULL DEFAULT now(),
  finished_at timestamptz,
  status      text NOT NULL DEFAULT 'running' CHECK (status IN ('running','success','failed','cancelled','abandoned')),
  detail      jsonb NOT NULL DEFAULT '{}'::jsonb CHECK (octet_length(detail::text) <= 8192),
  CHECK (finished_at IS NULL OR finished_at >= started_at)
);
CREATE INDEX job_runs_job_started_idx ON app.job_runs (job, started_at DESC);
CREATE INDEX job_runs_last_success_idx ON app.job_runs (job, finished_at DESC) WHERE status = 'success';

CREATE TABLE app.audit_log (
  id           bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  at           timestamptz NOT NULL DEFAULT now(),
  actor_id     uuid,            -- no FK: rows must survive user deletion (section 5.12)
  action       text NOT NULL CHECK (action ~ '^[a-z][a-z_]*(\.[a-z][a-z_]*)*$'),   -- dotted, e.g. follow.approved
  subject_type text NOT NULL CHECK (subject_type IN ('user','follow','credential','station')),
  subject_id   uuid,
  detail       jsonb NOT NULL DEFAULT '{}'::jsonb CHECK (octet_length(detail::text) <= 4096)
);
CREATE INDEX audit_log_at_idx ON app.audit_log (at DESC);
CREATE INDEX audit_log_subject_idx ON app.audit_log (subject_id, at DESC);

CREATE TABLE app.connect_attempts (
  id           bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  user_id      uuid NOT NULL REFERENCES app.users (id) ON DELETE CASCADE,
  attempted_at timestamptz NOT NULL DEFAULT now(),
  outcome      text NOT NULL CHECK (outcome IN ('started','valid','invalid','rate_limited','error'))
);   -- 02 inserts 'started' BEFORE calling AWN, then updates the outcome
CREATE INDEX connect_attempts_user_idx ON app.connect_attempts (user_id, attempted_at DESC);

-- Slot reservations for shared AWN rate gates (00-conventions, core::ratelimit).
CREATE TABLE app.rate_gates (
  name            text PRIMARY KEY CHECK (char_length(name) BETWEEN 1 AND 100),   -- awn:app:poller, awn:app:api, awn:key:<credential_id>, acis:api
  next_allowed_at timestamptz NOT NULL
);

GRANT SELECT ON app.job_runs TO app_api;
GRANT SELECT, INSERT, UPDATE, DELETE ON app.job_runs TO app_poller;   -- uuid key: no sequence grant needed
GRANT INSERT ON app.audit_log TO app_api, app_poller;           -- append only; neither can read or alter
GRANT SELECT, INSERT, DELETE ON app.connect_attempts TO app_api;
GRANT UPDATE (outcome) ON app.connect_attempts TO app_api;          -- 'started' -> final outcome
GRANT SELECT, INSERT, UPDATE ON app.rate_gates TO app_api, app_poller;
-- Belt and braces for identity sequences (privilege is normally not needed for GENERATED ... AS IDENTITY; tested):
GRANT USAGE ON SEQUENCE app.audit_log_id_seq TO app_api, app_poller;
GRANT USAGE ON SEQUENCE app.connect_attempts_id_seq TO app_api;
