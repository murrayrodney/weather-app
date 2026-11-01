import { defineConfig, devices } from '@playwright/test';

const baseURL = process.env.E2E_BASE_URL ?? 'http://localhost:8000';
const host = new URL(baseURL).hostname;
if (host !== 'localhost' && host !== '127.0.0.1') {
  throw new Error(`E2E_BASE_URL must be localhost or 127.0.0.1, got ${host}`);
}

export default defineConfig({
  testDir: './tests/e2e',
  outputDir: './test-results',
  reporter: [['list'], ['html', { outputFolder: 'playwright-report', open: 'never' }]],
  workers: 1,
  retries: 0,
  timeout: 60000,
  use: { baseURL, trace: 'retain-on-failure' },
  projects: [
    { name: 'setup', testMatch: /auth\.setup\.ts/ },
    { name: 'chromium', use: { ...devices['Desktop Chrome'], viewport: { width: 1400, height: 900 } }, dependencies: ['setup'], testIgnore: /auth\.setup\.ts/ }
  ]
});
