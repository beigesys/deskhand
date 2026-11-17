import * as React from 'react';
import { useNavigate, useSearch } from '@tanstack/react-router';
import { useQueryClient } from '@tanstack/react-query';
import { api, ApiError } from '@/api/client';
import { keys, useAuthInfo } from '@/api/hooks';
import type { Me } from '@/api/types';
import { Button } from '@/components/ui/button';
import { Field, Input } from '@/components/ui/form';
import { Logo } from '@/components/layout/AppShell';

function AuthFrame({ title, children }: { title: string; children: React.ReactNode }) {
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

function ErrorLine({ msg }: { msg: string | null }) {
	if (!msg) return null;
	return (
		<div role="alert" className="mb-3 rounded border border-bad/40 bg-bad/10 px-3 py-2 text-[13px] text-bad">
			{msg}
		</div>
	);
}

export function LoginPage() {
	const { next, bad } = useSearch({ from: '/login' });
	const info = useAuthInfo();
	const navigate = useNavigate();
	const qc = useQueryClient();
	const [error, setError] = React.useState<string | null>(bad ? 'Sign-in failed.' : null);
	const [busy, setBusy] = React.useState(false);

	React.useEffect(() => {
		if (info.data?.setup_required) navigate({ to: '/setup', search: { token: undefined } });
	}, [info.data, navigate]);

	async function submit(e: React.FormEvent<HTMLFormElement>) {
		e.preventDefault();
		const f = new FormData(e.currentTarget);
		setBusy(true);
		setError(null);
		try {
			const me = await api.post<Me>('/login', { username: f.get('username'), password: f.get('password') });
			qc.setQueryData(keys.me, me);
			navigate({ to: next && /^\/(?!\/|login|setup)/.test(next) ? next : '/' });
		} catch (err) {
			setError(err instanceof ApiError ? err.message : 'Sign-in failed.');
		} finally {
			setBusy(false);
		}
	}

	const providers = info.data?.providers ?? [];
	return (
		<AuthFrame title="Sign in">
			<ErrorLine msg={error} />
			{providers.length > 0 && (
				<div className="grid gap-2">
					{providers.map((p) => (
						<Button key={p.id} asChild variant="secondary" size="lg" className="justify-start">
							<a href={`/auth/${p.id}/start`}>Sign in with {p.label}</a>
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

	async function submit(e: React.FormEvent<HTMLFormElement>) {
		e.preventDefault();
		const f = new FormData(e.currentTarget);
		if (f.get('password') !== f.get('password2')) return setError("The passwords don't match.");
		try {
			const me = await api.post<Me>('/setup', {
				token: f.get('token'),
				username: f.get('username'),
				password: f.get('password'),
			});
			qc.setQueryData(keys.me, me);
			navigate({ to: '/' });
		} catch (err) {
			setError(err instanceof ApiError ? err.message : String(err));
		}
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
