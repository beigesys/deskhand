import type { Device } from '@/api/types';
import { Badge, Chip } from '@/components/ui/display';
import { attentionLabel } from '@/lib/utils';

/** The color of a device's left status bar. */
export function barColor(d: Pick<Device, 'online' | 'attention'>): string {
	if (!d.online) return 'var(--dh-off)';
	return d.attention.length ? 'var(--dh-warn)' : 'var(--dh-ok)';
}

export function StatusText({ d }: { d: Device }) {
	return d.online ? (
		<span className="text-ok">Online</span>
	) : (
		<span className="text-off">Offline</span>
	);
}

export function AttentionBadges({ d, max = 3 }: { d: Pick<Device, 'attention'>; max?: number }) {
	if (!d.attention.length) return null;
	return (
		<span className="inline-flex flex-wrap gap-1">
			{d.attention.slice(0, max).map((a) => (
				<Badge key={a} tone={a === 'offline_7d' ? 'off' : 'warn'}>
					{attentionLabel[a] ?? a}
				</Badge>
			))}
			{d.attention.length > max && <Badge>+{d.attention.length - max}</Badge>}
		</span>
	);
}

export function OsChip({ d }: { d: Pick<Device, 'os'> }) {
	return <Chip title={d.os}>{d.os}</Chip>;
}

/** The attention reasons as one colored line: red for security, amber for the rest. */
export function AttentionFlag({ d }: { d: Pick<Device, 'attention'> }) {
	if (!d.attention.length) return <span className="text-muted">—</span>;
	const bad = d.attention.some((a) => a === 'firewall_off' || a === 'av_off');
	return (
		<span className={bad ? 'text-[11.5px] font-medium text-bad' : 'text-[11.5px] font-medium text-warn'}>
			{d.attention.map((a) => attentionLabel[a] ?? a).join(' · ')}
		</span>
	);
}
