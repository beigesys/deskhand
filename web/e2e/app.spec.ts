import { expect, test, type Page } from '@playwright/test';

const user = process.env.DESKHAND_USER ?? 'admin';
const pass = process.env.DESKHAND_PASS ?? 'admin-password';
const device = process.env.DESKHAND_DEVICE ?? '';
const shots = process.env.DESKHAND_SHOTS;

async function shot(page: Page, name: string) {
	if (shots) await page.screenshot({ path: `${shots}/${name}.png`, fullPage: true });
}

async function signIn(page: Page) {
	if (process.env.PW_DEBUG_NET) page.on('response', (r) => console.log('resp:', r.status(), r.request().method(), r.url()));
	await page.goto('/');
	await expect(page).toHaveURL(/\/login/);
	await page.getByLabel('User name').fill(user);
	await page.getByLabel('Password').fill(pass);
	await page.getByRole('button', { name: 'Sign in' }).click();
	await expect(page.getByRole('heading', { name: 'Overview' })).toBeVisible();
}

test('sign in, see the live device, run a script and read its output', async ({ page }) => {
	await page.goto('/login');
	await shot(page, '01-login');
	await signIn(page);
	await expect(page.getByText('Online', { exact: true }).first()).toBeVisible();
	await shot(page, '02-dashboard');

	await page.getByRole('link', { name: /Devices/ }).click();
	const row = page.getByRole('row').filter({ hasText: device });
	await expect(row).toBeVisible();
	await expect(row.getByText('Online')).toBeVisible();
	await expect(row.getByRole('link', { name: 'Desktop' })).toBeVisible();
	await shot(page, '03-devices');

	await row.getByRole('link', { name: device }).click();
	await expect(page.getByRole('heading', { name: device })).toBeVisible();
	await expect(page.getByRole('tab', { name: 'Hardware' })).toBeVisible();
	// Inventory arrives a moment after the agent connects.
	await expect(async () => {
		await page.reload();
		await page.getByRole('tab', { name: 'Hardware' }).click();
		await expect(page.getByText('Cores / threads')).toBeVisible({ timeout: 1000 });
	}).toPass({ timeout: 20_000 });
	await page.getByRole('tab', { name: 'Overview' }).click();
	await shot(page, '04-device');

	// A script, then run it on this device and watch the output come back.
	await page.getByRole('link', { name: 'Scripts', exact: true }).click();
	await page.getByRole('link', { name: 'New script' }).click();
	await page.getByLabel('Name').fill('Who am I');
	await page.getByLabel('Shell').selectOption('bash');
	const editor = page.locator('.cm-content');
	await editor.click();
	await page.keyboard.press('Control+A');
	await page.keyboard.type('echo "hello-from-$(hostname)"; id -un');
	await shot(page, '05-script');
	await page.getByRole('button', { name: 'Save' }).click();
	await expect(page.getByRole('button', { name: 'Run…' })).toBeEnabled();

	await page.getByRole('button', { name: 'Run…' }).click();
	const dialog = page.getByRole('dialog');
	await dialog.getByRole('combobox', { name: 'Devices' }).click();
	await page.getByPlaceholder('Search name, IP, serial…').fill(device);
	await page.getByRole('option', { name: new RegExp(device) }).click();
	await page.keyboard.press('Escape');
	await expect(dialog.getByRole('combobox', { name: 'Devices' })).toContainText(device);
	await dialog.getByRole('button', { name: 'Run now' }).click();
	await expect(page.getByRole('heading', { name: /Who am I/ })).toBeVisible();
	await expect(page.locator('pre').filter({ hasText: 'hello-from-' })).toBeVisible({ timeout: 20_000 });
	await expect(page.getByText('ok', { exact: true }).first()).toBeVisible();
	await shot(page, '06-job');

	// Admin pages render.
	for (const [link, heading] of [
		['Groups', 'Groups'],
		['Enrollment', 'Enrollment'],
		['Users', 'Users'],
		['Audit', 'Audit log'],
	]) {
		await page.getByRole('navigation', { name: 'Main' }).getByRole('link', { name: link }).click();
		await expect(page.getByRole('heading', { name: heading })).toBeVisible();
	}
	await expect(page.locator('td').getByText('job.run').first()).toBeVisible();
	await shot(page, '07-audit');
	await page.getByRole('link', { name: 'Style guide' }).click();
	await expect(page.getByRole('heading', { name: 'Style guide' })).toBeVisible();
	await shot(page, '08-style');

	// Dark theme.
	await page.emulateMedia({ colorScheme: 'dark' });
	await page.getByRole('link', { name: /Devices/ }).click();
	await expect(row).toBeVisible();
	await shot(page, '09-devices-dark');
});

test('phone: drawer navigation and device cards', async ({ page }) => {
	await page.setViewportSize({ width: 390, height: 844 });
	await signIn(page);
	await page.getByRole('button', { name: 'Open menu' }).click();
	const drawer = page.getByRole('dialog', { name: 'Navigation' });
	await expect(drawer).toBeVisible();
	await expect(drawer.getByRole('link', { name: /Devices/ })).toBeVisible();
	// Tap targets in the drawer are at least 40px tall.
	const box = await drawer.getByRole('link', { name: /Devices/ }).boundingBox();
	expect(box!.height).toBeGreaterThanOrEqual(40);
	await shot(page, '11-phone-drawer');
	await page.keyboard.press('Escape');
	await expect(drawer).toBeHidden();

	await page.getByRole('button', { name: 'Open menu' }).click();
	await page.getByRole('dialog', { name: 'Navigation' }).getByRole('link', { name: /Devices/ }).click();
	await expect(page.getByRole('dialog', { name: 'Navigation' })).toBeHidden();
	const card = page.getByTestId('device-card').filter({ hasText: device });
	await expect(card).toBeVisible();
	await expect(page.locator('table')).toBeHidden();
	await expect(card.getByRole('link', { name: 'Desktop' })).toBeVisible();
	await shot(page, '12-phone-devices');

	// Select mode from the top bar.
	await page.getByRole('button', { name: 'Select' }).click();
	await card.getByRole('checkbox').click();
	await expect(page.getByText('1 selected').filter({ visible: true })).toBeVisible();
	await shot(page, '13-phone-select');
	await page.getByRole('button', { name: 'Done' }).click();

	// Overview tiles go 2×2.
	await page.getByRole('button', { name: 'Open menu' }).click();
	await page.getByRole('dialog', { name: 'Navigation' }).getByRole('link', { name: /Overview/ }).click();
	const tiles = page.getByRole('region', { name: 'Fleet status' }).locator(':scope > *');
	const [a, b, c] = await Promise.all([0, 1, 2].map((i) => tiles.nth(i).boundingBox()));
	expect(Math.round(a!.y)).toBe(Math.round(b!.y));
	expect(c!.y).toBeGreaterThan(a!.y);
	await shot(page, '14-phone-overview');
});
