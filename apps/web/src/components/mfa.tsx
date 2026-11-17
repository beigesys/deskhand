import * as React from 'react';
import { Check, Copy, Download } from 'lucide-react';
import type { MfaEnrollment } from '@/api/types';
import { Button } from '@/components/ui/button';
import { Field, Input } from '@/components/ui/form';

function useCopied() {
	const [copied, setCopied] = React.useState(false);
	React.useEffect(() => {
		if (!copied) return;
		const t = window.setTimeout(() => setCopied(false), 1500);
		return () => window.clearTimeout(t);
	}, [copied]);
	return {
		copied,
		copy: (text: string) => navigator.clipboard?.writeText(text).then(() => setCopied(true)),
	};
}

/** The QR code to scan, the secret to type in instead, and the code field (`name="code"`). */
export function QrSetup({ enrollment }: { enrollment: MfaEnrollment | null }) {
	const { copied, copy } = useCopied();
	if (!enrollment) return <div className="grid h-52 place-items-center text-[13px] text-muted">Preparing…</div>;
	const secret = enrollment.secret.replace(/(.{4})/g, '$1 ').trim();
	return (
		<div className="grid gap-3">
			<p className="text-[13px] text-muted">
				Scan this with an authenticator app (1Password, Bitwarden, Google Authenticator, Authy…), then enter the 6-digit
				code it shows.
			</p>
			<div className="flex flex-wrap items-center gap-4">
				<img
					src={`data:image/svg+xml;charset=utf-8,${encodeURIComponent(enrollment.qr_svg)}`}
					alt="QR code for your authenticator app"
					className="size-44 rounded border border-line bg-white p-1"
				/>
				<div className="min-w-0 flex-1">
					<div className="label-caps">Or type this key</div>
					<div className="mt-1 flex items-start gap-1">
						<code className="break-all font-mono text-[13px]">{secret}</code>
						<Button type="button" size="icon" variant="ghost" aria-label="Copy key" onClick={() => copy(enrollment.secret)}>
							{copied ? <Check /> : <Copy />}
						</Button>
					</div>
				</div>
			</div>
			<CodeField label="Code from the app" />
		</div>
	);
}

/** A one-time-code input named `code`. With `recovery`, it also takes recovery codes. */
export function CodeField({ label = 'Code', recovery = false, hint }: { label?: string; recovery?: boolean; hint?: string }) {
	return (
		<Field label={label} hint={hint}>
			<Input
				name="code"
				required
				autoFocus
				autoComplete="one-time-code"
				inputMode={recovery ? 'text' : 'numeric'}
				pattern={recovery ? undefined : '[0-9 ]{6,7}'}
				maxLength={recovery ? 16 : 7}
				placeholder={recovery ? '123456 or xxxxx-xxxxx' : '123456'}
				className="font-mono tracking-widest"
			/>
		</Field>
	);
}

/** Freshly made recovery codes: shown once, to copy or save. */
export function RecoveryCodes({ codes }: { codes: string[] }) {
	const { copied, copy } = useCopied();
	const text = codes.join('\n');
	function download() {
		const a = document.createElement('a');
		a.href = URL.createObjectURL(new Blob([`Deskhand recovery codes (${location.host})\n\n${text}\n`], { type: 'text/plain' }));
		a.download = 'deskhand-recovery-codes.txt';
		a.click();
		URL.revokeObjectURL(a.href);
	}
	return (
		<div className="grid gap-3">
			<p className="text-[13px] text-muted">
				Each code signs you in once if you lose your phone. Save them somewhere safe now: they won't be shown again.
			</p>
			<ol className="grid grid-cols-2 gap-x-6 gap-y-1 rounded border border-line bg-surface-2 p-3 font-mono text-[13px]">
				{codes.map((c) => (
					<li key={c}>{c}</li>
				))}
			</ol>
			<div className="flex gap-2">
				<Button type="button" variant="secondary" onClick={() => copy(text)}>
					{copied ? <Check /> : <Copy />} Copy
				</Button>
				<Button type="button" variant="secondary" onClick={download}>
					<Download /> Download
				</Button>
			</div>
		</div>
	);
}
