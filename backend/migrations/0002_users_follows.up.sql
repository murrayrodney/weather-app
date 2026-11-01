-- 0002 users_follows (04 section 5.2)
CREATE TABLE app.users (
  id            uuid PRIMARY KEY,                       -- = auth.users.id; NO foreign key (integrity is application-level)
  display_name  text NOT NULL CHECK (char_length(btrim(display_name)) BETWEEN 1 AND 80),
  is_active     boolean NOT NULL DEFAULT true,
  is_admin      boolean NOT NULL DEFAULT false,
  deactivated_at timestamptz,
  created_at    timestamptz NOT NULL DEFAULT now(),
  updated_at    timestamptz NOT NULL DEFAULT now(),
  CHECK (is_active = (deactivated_at IS NULL))
);
-- G4: at most one admin row (Rodney).
CREATE UNIQUE INDEX users_single_admin ON app.users ((true)) WHERE is_admin;
-- Display names identify people in the UI and in admin commands: unique, case- and space-insensitive.
CREATE UNIQUE INDEX users_display_name_key ON app.users (lower(btrim(display_name)));
-- Follow discovery (02 `GET /api/v1/users/search`): case-insensitive PREFIX match on the same normalised expression.
-- text_pattern_ops is what lets a btree serve `LIKE 'abc%'` under a non-C collation; partial because search never returns inactive users
-- (the query must repeat `is_active` and `lower(btrim(display_name))` literally to use it).
CREATE INDEX users_display_name_prefix_idx ON app.users (lower(btrim(display_name)) text_pattern_ops) WHERE is_active;
CREATE TRIGGER users_touch BEFORE UPDATE ON app.users FOR EACH ROW EXECUTE FUNCTION app.touch_updated_at();

CREATE TABLE app.follows (
  follower_id  uuid NOT NULL REFERENCES app.users (id) ON DELETE CASCADE,
  followee_id  uuid NOT NULL REFERENCES app.users (id) ON DELETE CASCADE,
  status       text NOT NULL DEFAULT 'pending'
               CHECK (status IN ('pending','approved','declined','revoked')),
  requested_at timestamptz NOT NULL DEFAULT now(),
  decided_at   timestamptz,
  updated_at   timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (follower_id, followee_id),
  CHECK (follower_id <> followee_id),
  CHECK ((status = 'pending') = (decided_at IS NULL))
);
CREATE INDEX follows_followee_idx ON app.follows (followee_id, status);
CREATE TRIGGER follows_touch BEFORE UPDATE ON app.follows FOR EACH ROW EXECUTE FUNCTION app.touch_updated_at();

GRANT SELECT ON app.users TO app_api;
GRANT SELECT (id, is_active) ON app.users TO app_poller;
GRANT SELECT, INSERT ON app.follows TO app_api;
GRANT UPDATE (status, decided_at) ON app.follows TO app_api;
