import * as React from 'react';
import { cn } from '@/lib/utils';

/** A bordered panel with an optional caps header and actions. */
export function Panel({
	title,
	actions,
	children,
	className,
	bodyClassName,
}: {
	title?: React.ReactNode;
	actions?: React.ReactNode;
	children: React.ReactNode;
	className?: string;
	bodyClassName?: string;
}) {
	return (
		<section className={cn('rounded border border-line bg-surface', className)}>
			{(title || actions) && (
				<header className="flex min-h-9 items-center justify-between gap-2 border-b border-line px-3">
					<h2 className="label-caps">{title}</h2>
					<div className="flex items-center gap-1.5">{actions}</div>
				</header>
			)}
			<div className={cn('p-3', bodyClassName)}>{children}</div>
		</section>
	);
}

/** Instrument-panel tile: a large tabular number with a small caps label. */
export function Gauge({
	label,
	value,
	tone,
	sub,
	onClick,
}: {
	label: string;
	value: React.ReactNode;
	tone?: 'ok' | 'off' | 'warn' | 'bad' | 'signal';
	sub?: React.ReactNode;
	onClick?: () => void;
}) {
	const color = {
		ok: 'text-ok',
		off: 'text-off',
		warn: 'text-warn',
		bad: 'text-bad',
		signal: 'text-signal',
	}[tone ?? 'off'];
	const Comp = onClick ? 'button' : 'div';
	return (
		<Comp
			onClick={onClick}
			className={cn(
				'relative flex flex-col items-start gap-1 rounded border border-line bg-surface px-4 py-3 text-left',
				onClick && 'cursor-pointer hover:border-line-strong',
			)}
		>
			<span className="label-caps">{label}</span>
			<span className={cn('font-mono text-3xl font-medium leading-none tabular', tone ? color : 'text-fg')}>
				{value}
			</span>
			{sub && <span className="text-xs text-muted">{sub}</span>}
			<span className={cn('absolute right-3 top-3 h-1.5 w-1.5 rounded-full', tone ? `bg-current ${color}` : 'bg-line')} />
		</Comp>
	);
}

const toneClasses = {
	ok: 'border-ok/40 bg-ok/10 text-ok',
	off: 'border-off/40 bg-off/10 text-off',
	warn: 'border-warn/40 bg-warn/10 text-warn',
	bad: 'border-bad/40 bg-bad/10 text-bad',
	run: 'border-run/40 bg-run/10 text-run',
	signal: 'border-signal/40 bg-signal-soft text-signal',
	neutral: 'border-line bg-surface-2 text-muted',
};
export type Tone = keyof typeof toneClasses;

export function Badge({ tone = 'neutral', children, className }: { tone?: Tone; children: React.ReactNode; className?: string }) {
	return (
		<span
			className={cn(
				'inline-flex h-5 items-center gap-1 rounded-sm border px-1.5 text-[11px] font-medium uppercase tracking-wide whitespace-nowrap',
				toneClasses[tone],
				className,
			)}
		>
			{children}
		</span>
	);
}

export function statusTone(s: string): Tone {
	switch (s) {
		case 'ok':
			return 'ok';
		case 'failed':
		case 'timeout':
			return 'bad';
		case 'running':
			return 'run';
		case 'queued':
			return 'warn';
		default:
			return 'off';
	}
}

export function StatusBadge({ status }: { status: string }) {
	return <Badge tone={statusTone(status)}>{status}</Badge>;
}

export function Chip({ children, title, className }: { children: React.ReactNode; title?: string; className?: string }) {
	return (
		<span className={cn('chip', className)} title={title}>
			{children}
		</span>
	);
}

/** Key/value rows. */
export function KV({ rows }: { rows: [string, React.ReactNode][] }) {
	return (
		<dl className="grid grid-cols-[minmax(110px,max-content)_1fr] gap-x-4 text-[13px]">
			{rows.map(([k, v]) => (
				<React.Fragment key={k}>
					<dt className="border-b border-line py-1.5 text-muted">{k}</dt>
					<dd className="min-w-0 border-b border-line py-1.5 break-words">{v ?? '—'}</dd>
				</React.Fragment>
			))}
		</dl>
	);
}

export function Empty({ children }: { children: React.ReactNode }) {
	return <div className="px-4 py-10 text-center text-[13px] text-muted">{children}</div>;
}

export function Meter({ used, total }: { used: number; total: number }) {
	const pct = total > 0 ? Math.min(100, (used / total) * 100) : 0;
	const tone = pct > 90 ? 'bg-bad' : pct > 75 ? 'bg-warn' : 'bg-navy-600';
	return (
		<div className="h-1.5 w-full min-w-16 overflow-hidden rounded-sm bg-surface-2" role="meter" aria-valuenow={Math.round(pct)}>
			<div className={cn('h-full', tone)} style={{ width: `${pct}%` }} />
		</div>
	);
}

export function PageHeader({
	title,
	sub,
	actions,
}: {
	title: React.ReactNode;
	sub?: React.ReactNode;
	actions?: React.ReactNode;
}) {
	return (
		<div className="flex flex-wrap items-end justify-between gap-3 pb-4">
			<div className="min-w-0">
				<h1 className="truncate text-xl font-semibold tracking-tight">{title}</h1>
				{sub && <div className="mt-0.5 text-[13px] text-muted">{sub}</div>}
			</div>
			{actions && <div className="flex flex-wrap items-center gap-2">{actions}</div>}
		</div>
	);
}
