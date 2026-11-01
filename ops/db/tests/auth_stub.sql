-- Stand-in for GoTrue's schema in throwaway databases (04 section 11.6). Idempotent. Includes columns the app must NOT reach.
-- Each role in its own block: parallel #[sqlx::test] databases race on these cluster-level roles.
DO $$ BEGIN CREATE ROLE anon NOLOGIN;          EXCEPTION WHEN duplicate_object OR unique_violation THEN NULL; END $$;
DO $$ BEGIN CREATE ROLE authenticated NOLOGIN; EXCEPTION WHEN duplicate_object OR unique_violation THEN NULL; END $$;
DO $$ BEGIN CREATE ROLE service_role NOLOGIN;  EXCEPTION WHEN duplicate_object OR unique_violation THEN NULL; END $$;
DO $$ BEGIN CREATE ROLE authenticator NOLOGIN; EXCEPTION WHEN duplicate_object OR unique_violation THEN NULL; END $$;
CREATE SCHEMA IF NOT EXISTS auth;
CREATE SCHEMA IF NOT EXISTS app AUTHORIZATION app_migrator;
CREATE TABLE IF NOT EXISTS auth.users (id uuid PRIMARY KEY, email text, encrypted_password text,
  raw_user_meta_data jsonb, banned_until timestamptz, deleted_at timestamptz, created_at timestamptz DEFAULT now());
CREATE TABLE IF NOT EXISTS auth.sessions (id uuid PRIMARY KEY, user_id uuid NOT NULL, not_after timestamptz,
  ip inet, user_agent text, factor_id uuid, created_at timestamptz DEFAULT now());
CREATE TABLE IF NOT EXISTS auth.identities (id uuid PRIMARY KEY, user_id uuid NOT NULL, identity_data jsonb);
CREATE TABLE IF NOT EXISTS auth.refresh_tokens (id bigserial PRIMARY KEY, token text, user_id text, session_id uuid);
