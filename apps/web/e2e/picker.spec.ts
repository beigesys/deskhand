import { expect, test } from '@playwright/test';

const user = process.env.DESKHAND_USER ?? 'admin';
const pass = process.env.DESKHAND_PASS ?? 'admin-password';
const shots = process.env.DESKHAND_SHOTS;

test('device picker over 5,000 devices: first page under 300ms, search narrows', async ({ page }) => {
	test.setTimeout(180_000);
	// Sign in through the API; the page shares the cookie.
	const login = await page.request.post('/api/v1/login', { data: { username: user, password: pass } });
	expect(login.ok()).toBeTruthy();
	const tok = await page.request.post('/api/v1/enroll-tokens', { data: { hours: 1, max_uses: 6000 } });
	const { token } = await tok.json();

	// Seed 5,000 devices through the enrollment endpoint, 64 at a time.
	const names = Array.from({ length: 5000 }, (_, i) => `PC-${String(i).padStart(5, '0')}`);
	for (let i = 0; i < names.length; i += 64) {
		await Promise.all(
			names.slice(i, i + 64).map((hostname) =>
				page.request.post('/api/enroll', { data: { token, hostname, os: 'windows' } }).then((r) => expect(r.ok()).toBeTruthy()),
			),
		);
	}

	await page.goto('/jobs');
	await page.getByRole('button', { name: 'New job' }).first().click();
	const dialog = page.getByRole('dialog');
	const picker = dialog.getByRole('combobox', { name: 'Devices' });
	const options = page.getByRole('option');

	// Opening loads the first page only.
	const t0 = Date.now();
	await picker.click();
	await expect(options.first()).toBeVisible();
	const took = Date.now() - t0;
	console.log(`device picker first page: ${took}ms`);
	expect(took).toBeLessThan(300);
	await expect(options).toHaveCount(25);
	if (shots) await page.screenshot({ path: `${shots}/15-picker.png` });

	// Scrolling to the bottom loads the next page.
	await page.locator('[cmdk-list]').evaluate((el) => el.scrollTo(0, el.scrollHeight));
	await expect(options).toHaveCount(50);

	// Searching narrows on the server.
	await page.getByPlaceholder('Search name, IP, serial…').fill('PC-0420');
	await expect(options).toHaveCount(10);
	await expect(options.first()).toContainText('PC-04200');
	await page.getByPlaceholder('Search name, IP, serial…').fill('PC-04217');
	await expect(options).toHaveCount(1);
	await options.first().click();
	await page.keyboard.press('Escape');
	await expect(picker).toContainText('PC-04217');
	if (shots) await page.screenshot({ path: `${shots}/16-picker-picked.png` });

	// The devices table pages too: 50 rows, and the count is the server's.
	await page.keyboard.press('Escape');
	await page.goto('/devices');
	await expect(page.getByText('5001 devices')).toBeVisible();
	await expect(page.locator('tbody tr')).toHaveCount(50);
});
