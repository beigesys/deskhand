import { defineConfig } from '@playwright/test';

// Run through scripts/e2e-web.sh, which starts a server and an agent and sets these.
export default defineConfig({
	testDir: 'e2e',
	timeout: 60_000,
	workers: 1,
	fullyParallel: false,
	reporter: 'list',
	use: {
		baseURL: process.env.DESKHAND_URL ?? 'http://127.0.0.1:8080',
		launchOptions: process.env.PW_CHROMIUM ? { executablePath: process.env.PW_CHROMIUM } : {},
		viewport: { width: 1360, height: 860 },
	},
});
