// Writes build/version.txt: the full commit SHA (COMMIT_REF on Netlify, else GITHUB_SHA), else the literal "local".
import { writeFileSync, mkdirSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';

const build = join(dirname(fileURLToPath(import.meta.url)), '..', 'build');
const raw = process.env.COMMIT_REF || process.env.GITHUB_SHA || '';
const value = /^[0-9a-f]{40}$/i.test(raw) ? raw.toLowerCase() : 'local';
mkdirSync(build, { recursive: true });
writeFileSync(join(build, 'version.txt'), value);
console.log(`version.txt: ${value}`);
