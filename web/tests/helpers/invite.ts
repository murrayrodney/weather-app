import { execFileSync } from 'node:child_process';
import type { Page } from '@playwright/test';
import { BASE_URL, REPO_ROOT } from './env';

function assertLocal() {
  const h = new URL(BASE_URL).hostname;
  if (h !== 'localhost' && h !== '127.0.0.1') throw new Error('invite helpers only run against the local stack');
}

function admin(args: string[]): string {
  assertLocal();
  return execFileSync('docker', ['compose', '--profile', 'tools', 'run', '--rm', '-T', 'admin', 'admin', ...args], {
    cwd: REPO_ROOT,
    encoding: 'utf8',
    stdio: ['ignore', 'pipe', 'pipe']
  });
}

function linkFrom(out: string): string {
  const lines = out.split('\n').filter((l) => l.startsWith(`${BASE_URL}/set-password#`));
  if (lines.length !== 1) throw new Error('expected exactly one set-password link');
  return lines[0].trim();
}

export function invite(email: string, name: string): string {
  return linkFrom(admin(['invite', '--email', email, '--display-name', name]));
}

export function resetLink(email: string): string {
  return linkFrom(admin(['reset-link', '--email', email]));
}

export function resetLinkFails(email: string): boolean {
  try {
    admin(['reset-link', '--email', email]);
    return false;
  } catch {
    return true;
  }
}

/** Opens the link in the page's context, fills the form and submits. */
export async function setPassword(page: Page, link: string, password: string) {
  await page.goto(link);
  await page.getByLabel('New password').fill(password);
  await page.getByLabel('Confirm password').fill(password);
  await page.getByRole('button', { name: 'Set password' }).click();
}

/** Display names are unique per user; make one per run. Letters only, so it passes name validation. */
export function uniqueName(prefix: string): string {
  const tag = Date.now().toString(36).replace(/[0-9]/g, (d) => String.fromCharCode(97 + Number(d)));
  return `${prefix} ${tag}`;
}

export function uniqueEmail(prefix: string): string {
  return `${prefix}-${Date.now()}-${Math.floor(Math.random() * 1000)}@example.test`;
}
