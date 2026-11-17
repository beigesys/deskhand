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
