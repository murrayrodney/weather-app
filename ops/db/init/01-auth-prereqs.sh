#!/bin/sh
set -eu
# GoTrue's own prerequisites; not app roles. The verifier is read inside psql with \getenv (not argv) and is a
# precomputed SCRAM-SHA-256 string, so no password text appears in SQL either.
psql -X -v ON_ERROR_STOP=1 -U "$POSTGRES_USER" -d "$POSTGRES_DB" <<'SQL'
\getenv auth_verifier SUPABASE_AUTH_ADMIN_PASSWORD_VERIFIER
-- Roles GoTrue's migrations and Supabase JWT claims refer to. NOLOGIN, no rights.
-- Created before roles.sql so its REVOKEs on anon/authenticated/service_role apply.
-- supabase_admin and dashboard_user are precautionary; delete them if auth boots without (05 8.1).
CREATE ROLE anon NOLOGIN NOINHERIT;
CREATE ROLE authenticated NOLOGIN NOINHERIT;
CREATE ROLE service_role NOLOGIN NOINHERIT BYPASSRLS;
CREATE ROLE authenticator NOLOGIN NOINHERIT; -- also in 04's test stub; creating it here avoids parallel-test races
CREATE ROLE supabase_admin NOLOGIN;
CREATE ROLE dashboard_user NOLOGIN;

CREATE ROLE supabase_auth_admin LOGIN NOINHERIT CREATEROLE PASSWORD :'auth_verifier';
GRANT CREATE ON DATABASE postgres TO supabase_auth_admin;
CREATE SCHEMA IF NOT EXISTS auth AUTHORIZATION supabase_auth_admin;
ALTER ROLE supabase_auth_admin SET search_path = auth;
SQL
