import { createHmac } from 'node:crypto';
import { expect, test, type Page } from '@playwright/test';

const admin = process.env.DESKHAND_USER ?? 'admin';
const adminPass = process.env.DESKHAND_PASS ?? 'admin-password';
const shots = process.env.DESKHAND_SHOTS;

async function shot(page: Page, name: string) {
	if (shots) await page.screenshot({ path: `${shots}/${name}.png`, fullPage: true });
}

/** RFC 6238 code for a base32 secret, `offset` steps from now. */
function totp(secret: string, offset = 0): string {
	const alphabet = 'ABCDEFGHIJKLMNOPQRSTUVWXYZ234567';
	let bits = '';
	for (const c of secret.replace(/\s/g, '')) bits += alphabet.indexOf(c.toUpperCase()).toString(2).padStart(5, '0');
	const key = Buffer.from(bits.match(/.{8}/g)!.map((b) => parseInt(b, 2)));
	const step = Buffer.alloc(8);
	step.writeBigUInt64BE(BigInt(Math.floor(Date.now() / 1000 / 30) + offset));
	const h = createHmac('sha1', key).update(step).digest();
	const o = h[19] & 15;
	return String((h.readUInt32BE(o) & 0x7fffffff) % 1_000_000).padStart(6, '0');
}

async function signIn(page: Page, user: string, pass: string) {
	await page.goto('/login');
	await page.getByLabel('User name').fill(user);
	await page.getByLabel('Password').fill(pass);
	await page.getByRole('button', { name: 'Sign in' }).click();
}

test('turn on two-step sign-in, sign in with a code and a recovery code, admin resets it', async ({ page, browser }) => {
	const name = `mfa-${Date.now().toString(36)}`;
	const pass = 'mfa-user-password';

	// An operator to try it on, made by the admin.
	await signIn(page, admin, adminPass);
	await expect(page.getByRole('heading', { name: 'Overview' })).toBeVisible();
	const made = await page.request.post('/api/v1/users', { data: { username: name, password: pass, role: 'operator' } });
	expect(made.ok()).toBeTruthy();

	const ctx = await browser.newContext();
	const p = await ctx.newPage();
	await signIn(p, name, pass);
	await expect(p.getByRole('heading', { name: 'Overview' })).toBeVisible();

	// Turn it on from the account page.
	await p.getByRole('button', { name: 'User settings' }).click();
	await p.getByRole('menuitem', { name: /two-step/ }).click();
	await expect(p.getByRole('heading', { name: 'Account' })).toBeVisible();
	await p.getByRole('button', { name: 'Turn on' }).click();
	const dialog = p.getByRole('dialog', { name: 'Turn on two-step sign-in' });
	await expect(dialog.getByAltText(/QR code/)).toBeVisible();
	const secret = (await dialog.locator('code').innerText()).replace(/\s/g, '');
	await shot(p, '20-mfa-enroll');
	await dialog.getByLabel('Code from the app').fill(totp(secret));
	await dialog.getByRole('button', { name: 'Turn on' }).click();
	// Hashing the recovery codes (argon2) takes a while in a debug build.
	await expect(dialog.getByRole('listitem')).toHaveCount(10, { timeout: 30_000 });
	const recovery = await dialog.getByRole('listitem').first().innerText();
	await shot(p, '21-mfa-codes');
	await dialog.getByRole('button', { name: 'Done' }).click();
	await expect(p.getByText('Recovery codes left')).toBeVisible();

	// Sign in again: the password leads to a code prompt. The enrollment code is spent, so
	// use the next step's.
	await ctx.clearCookies();
	await signIn(p, name, pass);
	await expect(p.getByRole('heading', { name: 'Two-step sign-in' })).toBeVisible();
	await shot(p, '22-mfa-login');
	await p.getByLabel(/Code from your authenticator/).fill('000000');
	await p.getByRole('button', { name: 'Verify' }).click();
	await expect(p.getByRole('alert')).toHaveText(/wrong code/);
	await p.getByLabel(/Code from your authenticator/).fill(totp(secret, 1));
	await p.getByRole('button', { name: 'Verify' }).click();
	await expect(p.getByRole('heading', { name: 'Overview' })).toBeVisible();

	// And with a recovery code.
	await ctx.clearCookies();
	await signIn(p, name, pass);
	await p.getByLabel(/Code from your authenticator/).fill(recovery);
	await p.getByRole('button', { name: 'Verify' }).click();
	await expect(p.getByRole('heading', { name: 'Overview' })).toBeVisible();

	// The admin sees it on, resets it, and the audit log shows all of it.
	await page.goto('/users');
	const row = page.getByRole('row').filter({ hasText: name });
	await expect(row.getByText('On', { exact: true })).toBeVisible();
	await row.getByRole('button', { name: `Reset two-step sign-in for ${name}` }).click();
	await page.getByRole('button', { name: 'Reset', exact: true }).click();
	await expect(row.getByText('Off', { exact: true })).toBeVisible();

	await page.goto('/audit');
	await page.getByLabel('Search').fill(name);
	for (const a of ['mfa.enable', 'mfa.recovery_used', 'mfa.reset', 'login.failed']) {
		await expect(page.getByText(a, { exact: true }).first()).toBeVisible();
	}
	await shot(page, '23-audit');
	await ctx.close();
});
