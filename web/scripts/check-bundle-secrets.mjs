// Scans build/ for secrets and unexpected hosts (01 section 6.4).
import { readdirSync, readFileSync, statSync, existsSync } from 'node:fs';
import { createHash } from 'node:crypto';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const build = join(root, 'build');
const errors = [];
const fail = (m) => errors.push(m);

function* walk(dir) {
  for (const name of readdirSync(dir)) {
    const p = join(dir, name);
    if (statSync(p).isDirectory()) yield* walk(p);
    else yield p;
  }
}

if (!existsSync(build)) {
  console.error('test:bundle FAIL: build/ missing (run npm run build)');
  process.exit(1);
}
const files = [...walk(build)];
const text = files.map((f) => readFileSync(f, 'utf8')).join('\n');

// JWTs
const jwts = new Set(text.match(/eyJ[A-Za-z0-9_-]+\.eyJ[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+/g) ?? []);
if (jwts.size > 1) fail(`more than one distinct JWT in the bundle (${jwts.size})`);
for (const j of jwts) {
  try {
    const payload = JSON.parse(Buffer.from(j.split('.')[1], 'base64url').toString('utf8'));
    if (payload.role !== 'anon') fail(`JWT in the bundle has role ${payload.role}`);
  } catch {
    fail('undecodable JWT-shaped string in the bundle');
  }
}

// supabase-js itself contains the bare prefix `sb_secret_` (a startsWith check), so only a prefix followed by key material fails.
if (/sb_secret_[A-Za-z0-9_-]{16,}/.test(text)) fail('bundle contains an sb_secret_ key');

const banned = ['service_role', 'BEGIN PRIVATE KEY', 'postgres://', 'postgresql://', 'AWN_APPLICATION_KEY', 'CREDENTIALS_PRIVATE_KEY', 'CREDENTIALS_PUBLIC_KEY', 'JWT_HS256_SECRET', 'SUPABASE_SERVICE_ROLE_KEY', 'NETLIFY_PROXY_SECRET', 'HEALTHCHECK_URL', 'AWS_SECRET'];
for (const b of banned) if (text.includes(b)) fail(`bundle contains ${b}`);

const url = process.env.PUBLIC_SUPABASE_URL ?? '';
const anon = process.env.PUBLIC_SUPABASE_ANON_KEY ?? '';
const production = process.env.CONTEXT === 'production';

if (production) {
  const fp = join(root, '..', 'backend', 'demo-fingerprints.txt');
  if (!existsSync(fp)) fail('production context: backend/demo-fingerprints.txt is missing');
  else {
    const h = createHash('sha256').update(anon).digest('hex');
    const hashes = readFileSync(fp, 'utf8').split('\n').map((l) => l.split(/\s+/)[0]);
    if (hashes.includes(h)) fail('production context: PUBLIC_SUPABASE_ANON_KEY is a demo value');
  }
  try {
    const host = new URL(url).hostname;
    if (['localhost', '127.0.0.1', '::1', '[::1]'].includes(host)) fail('production context: PUBLIC_SUPABASE_URL is a local host');
  } catch {
    fail('production context: PUBLIC_SUPABASE_URL is not a valid URL');
  }
}

// repo-root .env: secret-looking values must not appear in the build
const envPath = join(root, '..', '.env');
if (existsSync(envPath)) {
  for (const line of readFileSync(envPath, 'utf8').split('\n')) {
    const m = /^([A-Z0-9_]+)=(.*)$/.exec(line.trim());
    if (!m || m[1].startsWith('PUBLIC_')) continue;
    const value = m[2].replace(/^'|'$/g, '').replace(/^"|"$/g, '');
    const secretName = /(_PASSWORD|_SECRET|_KEY|_TOKEN)$/.test(m[1]) || /:\/\/[^/\s]*:[^@/\s]*@/.test(value);
    if (secretName && value.length >= 12 && text.includes(value)) fail(`the value of ${m[1]} from .env appears in the bundle`);
  }
}

// hosts
let supabaseHost = '';
try {
  supabaseHost = new URL(url).hostname;
} catch {
  /* reported below by the positive check */
}
const allowed = new Set(['localhost', supabaseHost, 'open-meteo.com', 'www.open-meteo.com', 'creativecommons.org', 'www.w3.org', 'svelte.dev', 'supabase.com', 'github.com'].filter(Boolean));
for (const m of text.matchAll(/https?:\/\/([A-Za-z0-9.-]+)/g)) {
  const host = m[1].replace(/\.$/, '');
  if (!allowed.has(host) && !host.endsWith('.supabase.com') && !host.endsWith('.github.com')) fail(`unexpected host in the bundle: ${host}`);
}

if (url && !text.includes(url)) fail('the build does not contain the value of PUBLIC_SUPABASE_URL');
if (!url) fail('PUBLIC_SUPABASE_URL is not set');

if (errors.length) {
  for (const e of [...new Set(errors)]) console.error(`test:bundle FAIL: ${e}`);
  process.exit(1);
}
console.log(`test:bundle ok (${files.length} files)`);
