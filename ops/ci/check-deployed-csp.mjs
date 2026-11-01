// The deployed 200.html must keep its inline-script hashes (06 section 13). Catches Netlify snippet injection or post-processing.
// SITE_URL=https://<site> node ops/ci/check-deployed-csp.mjs
import { createHash } from 'node:crypto';
const site = process.env.SITE_URL;
if (!site) { console.log('skip: SITE_URL unset'); process.exit(0); }
const html = await (await fetch(new URL('/200.html', site))).text();
const meta = html.match(/<meta[^>]+http-equiv="content-security-policy"[^>]*content="([^"]+)"/i);
if (!meta) { console.error('FAIL no CSP meta in deployed 200.html'); process.exit(1); }
const allowed = new Set([...meta[1].matchAll(/'sha256-([^']+)'/g)].map((m) => m[1]));
const bodies = [...html.matchAll(/<script(?![^>]*\bsrc=)[^>]*>([\s\S]*?)<\/script>/g)].map((m) => m[1]);
let bad = 0;
for (const b of bodies) {
  const h = createHash('sha256').update(b).digest('base64');
  if (!allowed.has(h)) { console.error('FAIL inline script not covered by CSP meta (injection or post-processing?)'); bad = 1; }
}
console.log(bad ? 'FAIL deployed CSP hashes' : `ok   deployed CSP hashes (${bodies.length} inline script(s))`);
process.exit(bad);
