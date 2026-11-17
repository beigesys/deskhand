import * as React from 'react';
import { createPortal } from 'react-dom';
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

/** Instrument panel: one bordered strip of tiles, 4 across (2×2 on phones). */
export function Tiles({ children, label }: { children: React.ReactNode; label?: string }) {
	return (
		<section
			aria-label={label}
			className="grid grid-cols-2 overflow-hidden rounded border border-line bg-surface lg:grid-cols-4 [&>*]:border-line [&>*:nth-child(even)]:border-l [&>*:nth-child(n+3)]:border-t lg:[&>*:nth-child(n+2)]:border-l lg:[&>*:nth-child(n+3)]:border-t-0"
		>
			{children}
		</section>
	);
}

const toneVar = { ok: 'var(--dh-ok)', off: 'var(--dh-off)', warn: 'var(--dh-warn)', bad: 'var(--dh-bad)', signal: 'var(--dh-signal)' };

/** One instrument tile: pip and caps label, a large tabular number, a thin gauge. */
export function Tile({
	label,
	value,
	of,
	tone = 'off',
	fill,
	sub,
	onClick,
}: {
	label: string;
	value: React.ReactNode;
	of?: React.ReactNode;
	tone?: keyof typeof toneVar;
	/** 0..1: how full the gauge under the number is. */
	fill?: number;
	sub?: React.ReactNode;
	onClick?: () => void;
}) {
	const Comp = onClick ? 'button' : 'div';
	return (
		<Comp
			onClick={onClick}
			style={{ ['--bar' as string]: toneVar[tone] }}
			className={cn(
				'flex min-w-0 flex-col gap-1.5 px-4 py-3.5 text-left',
				onClick && 'cursor-pointer hover:bg-signal/5',
			)}
		>
			<span className="label-caps flex items-center gap-1.5">
				<span className="pip" />
				{label}
			</span>
			<span className="font-mono text-[30px] font-medium leading-none tabular">
				{value}
				{of != null && <small className="text-sm font-normal text-muted"> / {of}</small>}
			</span>
			{fill != null && (
				<span className="block h-1 overflow-hidden rounded-sm bg-chip">
					<span className="block h-full" style={{ width: `${Math.min(100, Math.max(0, fill * 100))}%`, background: 'var(--bar)' }} />
				</span>
			)}
			{sub && <span className="text-xs text-muted">{sub}</span>}
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
	const tone = pct > 90 ? 'bg-bad' : pct > 75 ? 'bg-warn' : 'bg-muted';
	return (
		<div className="h-1 w-full min-w-16 overflow-hidden rounded-sm bg-chip" role="meter" aria-valuenow={Math.round(pct)}>
			<div className={cn('h-full', tone)} style={{ width: `${pct}%` }} />
		</div>
	);
}

/** Where a page puts its title and primary action in the phone top bar (AppShell). */
export const TopbarSlot = React.createContext<HTMLElement | null>(null);

/**
 * Page title row. On phones the title and `primary` move into the top bar; the other
 * actions stay here.
 */
export function PageHeader({
	title,
	sub,
	actions,
	primary,
}: {
	title: React.ReactNode;
	sub?: React.ReactNode;
	actions?: React.ReactNode;
	primary?: React.ReactNode;
}) {
	const slot = React.useContext(TopbarSlot);
	return (
		<>
			{slot &&
				createPortal(
					<div className="flex min-w-0 flex-1 items-center gap-2 md:hidden">
						<span className="min-w-0 flex-1 truncate text-[15px] font-semibold">{title}</span>
						{primary}
					</div>,
					slot,
				)}
			<div className="flex flex-wrap items-end justify-between gap-3 pb-4">
				<div className="min-w-0">
					<h1 className="truncate text-xl font-semibold tracking-tight max-md:sr-only">{title}</h1>
					{sub && <div className="mt-0.5 text-[13px] text-muted">{sub}</div>}
				</div>
				{(actions || primary) && (
					<div className="flex flex-wrap items-center gap-2">
						{actions}
						{primary && <span className="max-md:hidden">{primary}</span>}
					</div>
				)}
			</div>
		</>
	);
}
