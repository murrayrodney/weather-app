-- 0003 credentials (04 section 5.3)
CREATE TABLE app.credentials (
  id                 uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  owner_id           uuid NOT NULL REFERENCES app.users (id) ON DELETE CASCADE,
  api_key_ciphertext bytea NOT NULL CHECK (octet_length(api_key_ciphertext) BETWEEN 17 AND 512),
  encapsulated_key   bytea NOT NULL CHECK (octet_length(encapsulated_key) = 32),
  key_version        smallint NOT NULL CHECK (key_version >= 1),
  status             text NOT NULL DEFAULT 'ok' CHECK (status IN ('ok','invalid','rate_limited','replaced')),
  last_ok_at         timestamptz,
  status_changed_at  timestamptz NOT NULL DEFAULT now(),
  created_at         timestamptz NOT NULL DEFAULT now(),
  updated_at         timestamptz NOT NULL DEFAULT now(),
  CONSTRAINT credentials_id_owner_key UNIQUE (id, owner_id)   -- target of stations' composite FK (0005)
);
-- One live credential per owner (G6). Replaced rows are kept (history), so the uniqueness is partial.
CREATE UNIQUE INDEX credentials_one_live_per_owner ON app.credentials (owner_id) WHERE status <> 'replaced';
CREATE INDEX credentials_owner_idx ON app.credentials (owner_id);
CREATE INDEX credentials_key_version_idx ON app.credentials (key_version);
CREATE TRIGGER credentials_touch BEFORE UPDATE ON app.credentials FOR EACH ROW EXECUTE FUNCTION app.touch_updated_at();

-- app_api: metadata readable; ciphertext is insert-only. NO table-level SELECT, NO UPDATE of ciphertext columns.
GRANT SELECT (id, owner_id, key_version, status, last_ok_at, status_changed_at, created_at, updated_at)
  ON app.credentials TO app_api;
GRANT INSERT (id, owner_id, api_key_ciphertext, encapsulated_key, key_version) ON app.credentials TO app_api;
GRANT UPDATE (status, status_changed_at) ON app.credentials TO app_api;

-- app_poller: reads ciphertext; updates status and (for rotation) the ciphertext columns.
GRANT SELECT ON app.credentials TO app_poller;
GRANT UPDATE (status, status_changed_at, last_ok_at, api_key_ciphertext, encapsulated_key, key_version)
  ON app.credentials TO app_poller;
