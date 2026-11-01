import type { Page } from '@playwright/test';

/**
 * Records securitypolicyviolation events and CSP console messages for every document the page loads.
 * Violations are pushed to the Node side as they happen, so a later full navigation cannot hide an earlier one.
 */
export async function watchCsp(page: Page) {
  const consoleMsgs: string[] = [];
  const all: string[] = [];
  page.on('console', (m) => {
    if (m.text().includes('Content Security Policy')) consoleMsgs.push(m.text());
  });
  await page.exposeFunction('__reportCsp', (v: string) => {
    all.push(v);
  });
  await page.addInitScript(() => {
    document.addEventListener('securitypolicyviolation', (e) => {
      (window as unknown as { __reportCsp: (v: string) => void }).__reportCsp(`${e.violatedDirective} ${e.blockedURI} @ ${location.pathname}`);
    });
  });
  return {
    consoleMsgs,
    all,
    /** Everything recorded so far, across all documents. */
    violations: async () => {
      await page.waitForTimeout(50); // let in-flight reports arrive
      return [...all];
    },
    /** Assert nothing has been recorded yet (call after every navigation). */
    async expectClean(where: string) {
      const v = await this.violations();
      if (v.length || consoleMsgs.length) throw new Error(`CSP violations after ${where}: ${JSON.stringify([...v, ...consoleMsgs])}`);
    }
  };
}
