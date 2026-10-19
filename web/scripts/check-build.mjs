// Verifies build/200.html: one hashed inline bootstrap script, meta CSP, no inline styles/handlers,
// announcer hash matches security-headers.json, version.txt, NODE_VERSION == .nvmrc.
import { readFileSync, existsSync } from 'node:fs';
import { createHash } from 'node:crypto';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';
import { findAnnouncer } from './announcer.mjs';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const build = join(root, 'build');
const errors = [];
const fail = (m) => errors.push(m);

const htmlPath = join(build, '200.html');
if (!existsSync(htmlPath)) {
  console.error('check-build FAIL: build/200.html missing (run npm run build)');
  process.exit(1);
}
const html = readFileSync(htmlPath, 'utf8');

const meta = /<meta http-equiv="content-security-policy" content="([^"]*)"/i.exec(html);
if (!meta) fail('no CSP <meta> in 200.html');
const csp = meta?.[1] ?? '';
if (/unsafe-inline|unsafe-eval/.test(csp)) fail('meta CSP contains unsafe-inline/unsafe-eval');
if (!/script-src 'self'/.test(csp)) fail("meta CSP lacks script-src 'self'");

const scripts = [...html.matchAll(/<script\b([^>]*)>([\s\S]*?)<\/script>/gi)];
const inline = scripts.filter((m) => !/\bsrc=/.test(m[1]));
if (scripts.length !== inline.length) fail('200.html has an external <script src>');
if (inline.length !== 1) fail(`expected exactly one inline <script>, found ${inline.length}`);
else {
  const h = createHash('sha256').update(inline[0][2]).digest('base64');
  if (!csp.includes(`'sha256-${h}'`)) fail('the inline bootstrap script hash is not in the meta CSP');
}
if (/<style/i.test(html)) fail('200.html contains <style');
if (/\sstyle=/i.test(html)) fail('200.html contains a style= attribute');
if (/\son[a-z]+\s*=/i.test(html.replace(/<script[\s\S]*?<\/script>/gi, ''))) fail('200.html contains an on*= handler');

const ann = findAnnouncer(build);
if (!ann) fail('announcer style literal not found in build/_app (SvelteKit changed: re-evaluate the CSP exception)');
else {
  const cfg = JSON.parse(readFileSync(join(root, 'security-headers.json'), 'utf8'));
  if (!(cfg.csp['style-src-attr'] ?? []).includes(`'sha256-${ann.hash}'`)) {
    fail(`announcer hash differs from security-headers.json (run: npm run headers:generate -- --update-announcer-hash); built hash sha256-${ann.hash}`);
  }
}

const vp = join(build, 'version.txt');
if (!existsSync(vp)) fail('build/version.txt missing');
else if (!/^([0-9a-f]{40}|local)$/.test(readFileSync(vp, 'utf8').trim())) fail('build/version.txt is neither a 40-hex SHA nor "local"');

const nvm = readFileSync(join(root, '.nvmrc'), 'utf8').trim();
const toml = readFileSync(join(root, 'netlify.toml'), 'utf8');
const nv = /NODE_VERSION\s*=\s*"(\d+)"/.exec(toml)?.[1];
if (nv !== nvm) fail(`netlify.toml NODE_VERSION (${nv}) != .nvmrc (${nvm})`);

if (errors.length) {
  for (const e of errors) console.error(`check-build FAIL: ${e}`);
  process.exit(1);
}
console.log('check-build ok');
