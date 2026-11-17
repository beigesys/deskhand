import * as React from 'react';
import { KeyRound, ShieldCheck, ShieldOff } from 'lucide-react';
import { api } from '@/api/client';
import { keys, useMe, useMfa } from '@/api/hooks';
import type { MfaEnrollment } from '@/api/types';
import { CodeField, QrSetup, RecoveryCodes } from '@/components/mfa';
import { Button } from '@/components/ui/button';
import { Badge, Empty, KV, PageHeader, Panel } from '@/components/ui/display';
import { FormDialog } from '@/components/ui/form-dialog';
import { datetime } from '@/lib/utils';

type Codes = { recovery_codes: string[] };

/** The secret to scan, fetched each time the dialog opens. */
function EnrollDialog({ open, onOpenChange }: { open: boolean; onOpenChange: (v: boolean) => void }) {
	const [enrollment, setEnrollment] = React.useState<MfaEnrollment | null>(null);
	React.useEffect(() => {
		if (!open) return;
		setEnrollment(null);
		api.post<MfaEnrollment>('/me/mfa/enroll').then(setEnrollment, () => onOpenChange(false));
	}, [open, onOpenChange]);
	return (
		<FormDialog<Codes>
			open={open}
			onOpenChange={onOpenChange}
			title="Turn on two-step sign-in"
			submitLabel="Turn on"
			success="Two-step sign-in is on"
			invalidate={[keys.mfa]}
			action={(f) => api.post<Codes>('/me/mfa/confirm', { code: String(f.get('code') ?? '') })}
			successView={(r) => <RecoveryCodes codes={r.recovery_codes} />}
		>
			<QrSetup enrollment={enrollment} />
		</FormDialog>
	);
}

export function AccountPage() {
	const me = useMe();
	const mfa = useMfa();
	const [enrolling, setEnrolling] = React.useState(false);
	const [regenerating, setRegenerating] = React.useState(false);
	const [disabling, setDisabling] = React.useState(false);
	const s = mfa.data;
	return (
		<>
			<PageHeader title="Account" sub={me.data ? `Signed in as ${me.data.username} (${me.data.role})` : undefined} />
			<Panel
				title="Two-step sign-in"
				className="max-w-2xl"
				actions={s && s.password_user && (s.enabled ? <Badge tone="ok">On</Badge> : <Badge tone="warn">Off</Badge>)}
			>
				{!s ? (
					<Empty>{mfa.isLoading ? 'Loading…' : 'Couldn’t load your settings.'}</Empty>
				) : !s.password_user ? (
					<p className="text-[13px] text-muted">
						You sign in with single sign-on, so your identity provider handles two-step verification.
					</p>
				) : s.enabled ? (
					<div className="grid gap-3">
						<KV
							rows={[
								['Authenticator app', `On since ${datetime(s.enabled_at)}`],
								['Recovery codes left', s.recovery_left < 3 ? <Badge tone="warn">{s.recovery_left}</Badge> : String(s.recovery_left)],
							]}
						/>
						<div className="flex flex-wrap gap-2">
							<Button onClick={() => setRegenerating(true)}>
								<KeyRound /> New recovery codes
							</Button>
							{!s.required && (
								<Button variant="ghost" className="text-bad" onClick={() => setDisabling(true)}>
									<ShieldOff /> Turn off
								</Button>
							)}
						</div>
						{s.required && <p className="text-xs text-muted">This server requires two-step sign-in, so it can't be turned off.</p>}
					</div>
				) : (
					<div className="grid gap-3">
						<p className="text-[13px] text-muted">
							After your password, sign-in will ask for a code from an authenticator app on your phone. A stolen password
							alone won't get anyone in.
						</p>
						<div>
							<Button variant="primary" onClick={() => setEnrolling(true)}>
								<ShieldCheck /> Turn on
							</Button>
						</div>
					</div>
				)}
			</Panel>

			<EnrollDialog open={enrolling} onOpenChange={setEnrolling} />
			<FormDialog<Codes>
				open={regenerating}
				onOpenChange={setRegenerating}
				title="New recovery codes"
				description="Your old recovery codes stop working."
				submitLabel="Make new codes"
				success="New recovery codes made"
				invalidate={[keys.mfa]}
				action={(f) => api.post<Codes>('/me/mfa/recovery-codes', { code: String(f.get('code') ?? '') })}
				successView={(r) => <RecoveryCodes codes={r.recovery_codes} />}
			>
				<CodeField label="Code from your authenticator app" />
			</FormDialog>
			<FormDialog
				open={disabling}
				onOpenChange={setDisabling}
				title="Turn off two-step sign-in?"
				description="Sign-in will only need your password."
				submitLabel="Turn off"
				success="Two-step sign-in is off"
				invalidate={[keys.mfa]}
				action={(f) => api.post('/me/mfa/disable', { code: String(f.get('code') ?? '') })}
			>
				<CodeField label="Code from your authenticator app" recovery />
			</FormDialog>
		</>
	);
}
