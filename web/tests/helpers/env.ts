import { existsSync, readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

export const REPO_ROOT = join(dirname(fileURLToPath(import.meta.url)), '..', '..', '..');

// The local demo password is a committed, public, local-only value (.env.example, fingerprinted in backend/demo-fingerprints.txt).
function demoPassword(): string {
  if (process.env.E2E_DEMO_PASSWORD) return process.env.E2E_DEMO_PASSWORD;
  for (const f of ['.env', '.env.example']) {
    const p = join(REPO_ROOT, f);
    if (!existsSync(p)) continue;
    const m = /^LOCAL_DEMO_PASSWORD=(.*)$/m.exec(readFileSync(p, 'utf8'));
    if (m) return m[1].replace(/^'|'$/g, '');
  }
  throw new Error('E2E_DEMO_PASSWORD is not set and LOCAL_DEMO_PASSWORD was not found');
}

export const BASE_URL = process.env.E2E_BASE_URL ?? 'http://localhost:8000';
export const USER_A_EMAIL = process.env.E2E_USER_A_EMAIL ?? 'alex@example.test';
export const USER_B_EMAIL = process.env.E2E_USER_B_EMAIL ?? 'pat@example.test';
export const USER_DANA_EMAIL = 'dana@example.test';
export const UNINVITED_EMAIL = process.env.E2E_UNINVITED_EMAIL ?? 'nobody@example.test';
export const DEMO_PASSWORD = demoPassword();
export const VALID_AWN_KEY = process.env.E2E_VALID_AWN_KEY ?? 'fixture-valid-key';
export const INVALID_AWN_KEY = process.env.E2E_INVALID_AWN_KEY ?? 'fixture-invalid-key';
export const AUTH_DIR = join(REPO_ROOT, 'web', 'tests', '.auth');
export const NEW_PASSWORD = 'a-brand-new-passphrase-1';
