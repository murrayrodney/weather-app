// Runs a command with PUBLIC_* filled from .env.test when missing (lint, check and unit tests have no real values).
// Usage: node scripts/with-test-env.mjs <command> [args...]
import { readFileSync } from 'node:fs';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { dirname, join, delimiter } from 'node:path';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const env = { ...process.env };
for (const line of readFileSync(join(root, '.env.test'), 'utf8').split('\n')) {
  const m = /^([A-Z_]+)=(.*)$/.exec(line.trim());
  if (m && !env[m[1]]) env[m[1]] = m[2];
}
env.PATH = join(root, 'node_modules', '.bin') + delimiter + (env.PATH ?? '');
const [cmd, ...args] = process.argv.slice(2);
if (!cmd) {
  console.error('usage: with-test-env.mjs <command> [args...]');
  process.exit(2);
}
const r = spawnSync(cmd, args, { stdio: 'inherit', env, cwd: root });
process.exit(r.status ?? 1);
