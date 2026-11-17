import type { Device } from '@/api/types';
import { Badge, Chip } from '@/components/ui/display';
import { Tip } from '@/components/ui/overlay';
import { ago, attentionLabel, span } from '@/lib/utils';

/** The color of a device's left status bar. */
export function barColor(d: Pick<Device, 'online' | 'attention'>): string {
	if (!d.online) return 'var(--dh-off)';
	return d.attention.length ? 'var(--dh-warn)' : 'var(--dh-ok)';
}

/** What the status colors mean, for a column header's tooltip. */
export const STATUS_LEGEND = (
	<span className="grid gap-1">
		{(
			[
				['var(--dh-ok)', 'Online'],
				['var(--dh-warn)', 'Online, needs attention'],
				['var(--dh-off)', 'Offline'],
			] as const
		).map(([c, l]) => (
			<span key={l} className="inline-flex items-center gap-1.5">
				<span className="pip" style={{ ['--bar' as string]: c }} />
				{l}
			</span>
		))}
	</span>
);

/**
 * A device's state as a pip and words that agree with it: green "Online", amber "Online ·
 * attention", grey "Offline 3d". The tooltip says why.
 */
export function StateText({ d }: { d: Pick<Device, 'online' | 'attention' | 'last_seen'> }) {
	const now = Math.floor(Date.now() / 1000);
	const why = d.attention.map((a) => attentionLabel[a] ?? a).join(', ');
	const tip = d.online
		? why
			? `Online; needs attention: ${why}`
			: 'Online'
		: `Offline; last seen ${ago(d.last_seen)}${why ? `. ${why}` : ''}`;
	return (
		<Tip label={tip}>
			<span className="inline-flex items-center gap-1.5 whitespace-nowrap text-[12.5px]" style={{ ['--bar' as string]: barColor(d) }}>
				<span className="pip" />
				{d.online ? (
					<>
						Online
						{d.attention.length > 0 && <span className="text-warn">· attention</span>}
					</>
				) : (
					<span className="text-muted">Offline {span(now - d.last_seen)}</span>
				)}
			</span>
		</Tip>
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

/** "outdated" next to a device whose agent version isn't the server's, or how its
 * self-update is going. */
export function AgentOutdated({
	d,
}: {
	d: Pick<Device, 'agent_outdated' | 'agent_version' | 'agent_update' | 'agent_update_detail'>;
}) {
	if (!d.agent_outdated) return null;
	if (d.agent_update === 'updating')
		return (
			<span title={`Updating the agent to ${d.agent_update_detail ?? "this server's version"}`}>
				<Badge>updating agent</Badge>
			</span>
		);
	if (d.agent_update === 'failed')
		return (
			<span title={d.agent_update_detail ?? 'The agent update failed'}>
				<Badge tone="bad">agent update failed</Badge>
			</span>
		);
	return (
		<span title={`Agent ${d.agent_version ?? 'unknown'} differs from this server's version`}>
			<Badge tone="warn">outdated agent</Badge>
		</span>
	);
}
