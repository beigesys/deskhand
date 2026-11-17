import * as React from 'react';
import { useNavigate, useSearch } from '@tanstack/react-router';
import { useQueryClient } from '@tanstack/react-query';
import { api, ApiError } from '@/api/client';
import { keys, useAuthInfo } from '@/api/hooks';
import type { LoginResult, Me, MfaEnrollment } from '@/api/types';
import { CodeField, QrSetup, RecoveryCodes } from '@/components/mfa';
import { Button } from '@/components/ui/button';
import { Field, Input } from '@/components/ui/form';
import { Logo } from '@/components/layout/AppShell';
import { rememberNext } from '@/lib/utils';

export function AuthFrame({ title, children }: { title: string; children: React.ReactNode }) {
	return (
		<div className="grid min-h-dvh md:grid-cols-[minmax(280px,420px)_1fr]">
			<div className="hidden flex-col justify-between border-r border-rail-border bg-rail p-8 text-rail-fg md:flex">
				<Logo className="text-rail-strong" />
				<div>
					<div className="label-caps !text-rail-dim">Remote management</div>
					<p className="mt-2 max-w-xs text-[15px] leading-relaxed text-rail-strong">
						Every machine in the fleet, one static binary on each.
					</p>
					<div className="mt-6 h-1 w-16 bg-signal" />
				</div>
				<div className="font-mono text-[11px] text-rail-dim">deskhand.sh</div>
			</div>
			<div className="grid place-items-center p-4">
				<div className="w-full max-w-sm">
					<Logo className="mb-6 md:hidden" />
					<h1 className="text-xl font-semibold">{title}</h1>
					<div className="mt-5">{children}</div>
				</div>
			</div>
		</div>
	);
}

export function ErrorLine({ msg }: { msg: string | null }) {
	if (!msg) return null;
	return (
		<div role="alert" className="mb-3 rounded border border-bad/40 bg-bad/10 px-3 py-2 text-[13px] text-bad">
			{msg}
		</div>
	);
}

type Step = { kind: 'verify'; token: string } | { kind: 'enroll'; token: string };

function stepOf(r: LoginResult): Step | null {
	if ('mfa_required' in r) return { kind: 'verify', token: r.mfa_token };
	if ('mfa_enroll_required' in r) return { kind: 'enroll', token: r.mfa_token };
	return null;
}

/** The second step after a correct password: a code, or setting up an authenticator first. */
function SecondStep({ step, onDone, onRestart }: { step: Step; onDone: (me: Me) => void; onRestart: () => void }) {
	const [error, setError] = React.useState<string | null>(null);
	const [busy, setBusy] = React.useState(false);
	const [enrollment, setEnrollment] = React.useState<MfaEnrollment | null>(null);
	const [codes, setCodes] = React.useState<{ me: Me; codes: string[] } | null>(null);

	React.useEffect(() => {
		if (step.kind !== 'enroll') return;
		api.post<MfaEnrollment>('/login/mfa/enroll', { mfa_token: step.token }).then(setEnrollment, (e) =>
			setError(e instanceof ApiError ? e.message : String(e)),
		);
	}, [step]);

	async function submit(e: React.FormEvent<HTMLFormElement>) {
		e.preventDefault();
		const code = String(new FormData(e.currentTarget).get('code') ?? '');
		setBusy(true);
		setError(null);
		try {
			if (step.kind === 'verify') {
				onDone(await api.post<Me>('/login/mfa', { mfa_token: step.token, code }));
			} else {
				const r = await api.post<{ me: Me; recovery_codes: string[] }>('/login/mfa/enroll/confirm', { mfa_token: step.token, code });
				setCodes({ me: r.me, codes: r.recovery_codes });
			}
		} catch (err) {
			setError(err instanceof ApiError ? err.message : 'That didn’t work.');
		} finally {
			setBusy(false);
		}
	}

	if (codes) {
		return (
			<div className="grid gap-3">
				<RecoveryCodes codes={codes.codes} />
				<Button variant="primary" size="lg" onClick={() => onDone(codes.me)}>
					I've saved them — continue
				</Button>
			</div>
		);
	}
	return (
		<>
			<ErrorLine msg={error} />
			<form onSubmit={submit} className="grid gap-3">
				{step.kind === 'verify' ? (
					<CodeField label="Code from your authenticator app" recovery hint="Lost your phone? Enter a recovery code instead." />
				) : (
					<>
						<p className="text-[13px] text-muted">This server requires two-step sign-in. Set it up to continue.</p>
						<QrSetup enrollment={enrollment} />
					</>
				)}
				<Button type="submit" variant="primary" size="lg" disabled={busy || (step.kind === 'enroll' && !enrollment)}>
					{busy ? 'Checking…' : step.kind === 'verify' ? 'Verify' : 'Turn on and sign in'}
				</Button>
				<Button type="button" variant="ghost" onClick={onRestart}>
					Start over
				</Button>
			</form>
		</>
	);
}

export function LoginPage() {
	const { next, bad } = useSearch({ from: '/login' });
	const info = useAuthInfo();
	const navigate = useNavigate();
	const qc = useQueryClient();
	const [error, setError] = React.useState<string | null>(bad ? 'Sign-in failed.' : null);
	const [busy, setBusy] = React.useState(false);
	const [step, setStep] = React.useState<Step | null>(null);

	React.useEffect(() => {
		if (info.data?.setup_required) navigate({ to: '/setup', search: { token: undefined } });
	}, [info.data, navigate]);

	function done(me: Me) {
		qc.setQueryData(keys.me, me);
		const to = next && /^\/(?!\/|login|setup)/.test(next) ? next : '/';
		// A query string (the MCP consent page's request) is a full page load away.
		if (to.includes('?')) window.location.assign(to);
		else navigate({ to });
	}

	async function submit(e: React.FormEvent<HTMLFormElement>) {
		e.preventDefault();
		const f = new FormData(e.currentTarget);
		setBusy(true);
		setError(null);
		try {
			const r = await api.post<LoginResult>('/login', { username: f.get('username'), password: f.get('password') });
			const s = stepOf(r);
			if (s) setStep(s);
			else done(r as Me);
		} catch (err) {
			setError(err instanceof ApiError ? err.message : 'Sign-in failed.');
		} finally {
			setBusy(false);
		}
	}

	if (step) {
		return (
			<AuthFrame title={step.kind === 'verify' ? 'Two-step sign-in' : 'Set up two-step sign-in'}>
				<SecondStep step={step} onDone={done} onRestart={() => setStep(null)} />
			</AuthFrame>
		);
	}

	const providers = info.data?.providers ?? [];
	return (
		<AuthFrame title="Sign in">
			<ErrorLine msg={error} />
			{providers.length > 0 && (
				<div className="grid gap-2">
					{providers.map((p) => (
						<Button key={p.id} asChild variant="secondary" size="lg" className="justify-start">
							<a href={`/auth/${p.id}/start`} onClick={() => rememberNext(next)}>
								Sign in with {p.label}
							</a>
						</Button>
					))}
				</div>
			)}
			{providers.length > 0 && info.data?.password_login && (
				<div className="my-4 flex items-center gap-3 text-xs text-subtle">
					<span className="h-px flex-1 bg-line" /> or <span className="h-px flex-1 bg-line" />
				</div>
			)}
			{info.data?.password_login !== false && (
				<form onSubmit={submit} className="grid gap-3">
					<Field label="User name">
						<Input name="username" autoComplete="username" required autoFocus />
					</Field>
					<Field label="Password">
						<Input name="password" type="password" autoComplete="current-password" required />
					</Field>
					<Button type="submit" variant="primary" size="lg" disabled={busy}>
						{busy ? 'Signing in…' : 'Sign in'}
					</Button>
				</form>
			)}
		</AuthFrame>
	);
}

export function SetupPage() {
	const { token } = useSearch({ from: '/setup' });
	const navigate = useNavigate();
	const qc = useQueryClient();
	const [error, setError] = React.useState<string | null>(null);
	const [step, setStep] = React.useState<Step | null>(null);

	function done(me: Me) {
		qc.setQueryData(keys.me, me);
		navigate({ to: '/' });
	}

	async function submit(e: React.FormEvent<HTMLFormElement>) {
		e.preventDefault();
		const f = new FormData(e.currentTarget);
		if (f.get('password') !== f.get('password2')) return setError("The passwords don't match.");
		try {
			const r = await api.post<LoginResult>('/setup', {
				token: f.get('token'),
				username: f.get('username'),
				password: f.get('password'),
			});
			const s = stepOf(r);
			if (s) setStep(s);
			else done(r as Me);
		} catch (err) {
			setError(err instanceof ApiError ? err.message : String(err));
		}
	}

	if (step) {
		return (
			<AuthFrame title="Set up two-step sign-in">
				<SecondStep step={step} onDone={done} onRestart={() => navigate({ to: '/login', search: {} })} />
			</AuthFrame>
		);
	}

	return (
		<AuthFrame title="Create the first admin">
			<p className="-mt-2 mb-4 text-[13px] text-muted">
				Use the setup link the server printed when it started.
			</p>
			<ErrorLine msg={error} />
			<form onSubmit={submit} className="grid gap-3">
				<Field label="Setup token">
					<Input name="token" defaultValue={token ?? ''} required className="font-mono" />
				</Field>
				<Field label="User name">
					<Input name="username" defaultValue="admin" required autoComplete="username" />
				</Field>
				<Field label="Password" hint="8 characters or more.">
					<Input name="password" type="password" minLength={8} required autoComplete="new-password" />
				</Field>
				<Field label="Repeat password">
					<Input name="password2" type="password" minLength={8} required autoComplete="new-password" />
				</Field>
				<Button type="submit" variant="primary" size="lg">
					Create admin
				</Button>
			</form>
		</AuthFrame>
	);
}
