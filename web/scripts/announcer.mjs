// Finds SvelteKit's route-announcer inline style literal in the built runtime and hashes it (CSP style-src-attr).
import { readdirSync, readFileSync, statSync } from 'node:fs';
import { createHash } from 'node:crypto';
import { join } from 'node:path';

const START = 'position: absolute; left: 0; top: 0; clip: rect(0 0 0 0)';

function* walk(dir) {
  for (const name of readdirSync(dir)) {
    const p = join(dir, name);
    if (statSync(p).isDirectory()) yield* walk(p);
    else if (p.endsWith('.js')) yield p;
  }
}

/** @returns {{ literal: string, hash: string } | null} */
export function findAnnouncer(buildDir) {
  for (const file of walk(join(buildDir, '_app'))) {
    const src = readFileSync(file, 'utf8');
    const i = src.indexOf(START);
    if (i < 0) continue;
    const quoteChars = ['"', "'", '`'];
    let end = -1;
    for (let j = i; j < src.length; j++) {
      if (quoteChars.includes(src[j])) {
        end = j;
        break;
      }
    }
    if (end < 0) continue;
    const literal = src.slice(i, end);
    return { literal, hash: createHash('sha256').update(literal).digest('base64') };
  }
  return null;
}
