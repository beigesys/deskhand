import { Link, useNavigate } from '@tanstack/react-router';
import { useJobs, useSummary } from '@/api/hooks';
import { AttentionFlag, barColor, OsChip } from '@/components/devices';
import { Empty, PageHeader, Panel, StatusBadge, Tile, Tiles } from '@/components/ui/display';
import { Table, Td, Th, Tr } from '@/components/ui/table';
import { ago, until } from '@/lib/utils';

export function DashboardPage() {
	const summary = useSummary();
	const jobs = useJobs(undefined, { limit: 25 });
	const navigate = useNavigate();
	const total = summary.data?.total ?? 0;
	const online = summary.data?.online ?? 0;
	const attentionCount = summary.data?.attention ?? 0;
	const attention = summary.data?.attention_devices ?? [];
	const failing = jobs.items.reduce((n, j) => n + (j.counts?.failed ?? 0) + (j.counts?.timeout ?? 0), 0);
	const running = jobs.items.reduce((n, j) => n + (j.counts?.running ?? 0) + (j.counts?.queued ?? 0), 0);
	const recent = jobs.items.slice(0, 8);

	return (
		<>
			<PageHeader title="Overview" sub={`${total} device${total === 1 ? '' : 's'} in the fleet`} />
			<Tiles label="Fleet status">
				<Tile
					label="Online"
					value={online}
					of={total}
					tone="ok"
					fill={total ? online / total : 0}
					onClick={() => navigate({ to: '/devices', search: { status: 'online' } })}
				/>
				<Tile
					label="Offline"
					value={total - online}
					tone="off"
					fill={total ? (total - online) / total : 0}
					onClick={() => navigate({ to: '/devices', search: { status: 'offline' } })}
				/>
				<Tile
					label="Needs attention"
					value={attentionCount}
					tone="warn"
					fill={total ? attentionCount / total : 0}
					onClick={() => navigate({ to: '/devices', search: { status: 'attention' } })}
				/>
				<Tile
					label="Failed runs"
					value={failing}
					tone="bad"
					fill={failing ? Math.min(1, failing / Math.max(1, total)) : 0}
					sub={running ? `${running} queued or running` : undefined}
					onClick={() => navigate({ to: '/jobs' })}
				/>
			</Tiles>

			<div className="mt-4 grid gap-4 xl:grid-cols-2">
				<Panel
					title={`Needs attention (${attentionCount})`}
					actions={
						attentionCount > attention.length && (
							<Link to="/devices" search={{ status: 'attention' }} className="text-xs text-muted hover:text-fg">
								All {attentionCount} →
							</Link>
						)
					}
					bodyClassName="p-0"
				>
					{attention.length === 0 ? (
						<Empty>Nothing needs a look right now.</Empty>
					) : (
						<Table>
							<thead>
								<tr>
									<Th>Device</Th>
									<Th>OS</Th>
									<Th>Why</Th>
								</tr>
							</thead>
							<tbody>
								{attention.map((d) => (
									<Tr key={d.id} className="status-bar" style={{ ['--bar' as string]: barColor(d) }}>
										<Td>
											<Link to="/devices/$id" params={{ id: d.id }} search={{ tab: undefined }} className="font-medium hover:text-signal">
												{d.name}
											</Link>
										</Td>
										<Td>
											<OsChip d={d} />
										</Td>
										<Td>
											<AttentionFlag d={d} />
										</Td>
									</Tr>
								))}
							</tbody>
						</Table>
					)}
				</Panel>

				<Panel
					title="Recent jobs"
					actions={
						<Link to="/jobs" className="text-xs text-muted hover:text-fg">
							All jobs →
						</Link>
					}
					bodyClassName="p-0"
				>
					{recent.length === 0 ? (
						<Empty>No jobs yet.</Empty>
					) : (
						<Table>
							<thead>
								<tr>
									<Th>Job</Th>
									<Th>Target</Th>
									<Th>Results</Th>
									<Th>When</Th>
								</tr>
							</thead>
							<tbody>
								{recent.map((j) => (
									<Tr key={j.id}>
										<Td className="max-w-56 truncate">
											<Link to="/jobs/$id" params={{ id: String(j.id) }} className="font-medium hover:text-signal">
												{j.name}
											</Link>
										</Td>
										<Td className="text-muted">{j.target}</Td>
										<Td>
											<span className="inline-flex gap-1">
												{j.counts && j.counts.ok > 0 && <StatusBadge status="ok" />}
												{j.counts && j.counts.failed + j.counts.timeout > 0 && <StatusBadge status="failed" />}
												{j.counts && j.counts.running + j.counts.queued > 0 && <StatusBadge status="running" />}
											</span>
										</Td>
										<Td className="text-muted">
											{j.schedule ? `next ${until(j.next_fire)}` : ago(j.last_fired ?? j.created)}
										</Td>
									</Tr>
								))}
							</tbody>
						</Table>
					)}
				</Panel>
			</div>
		</>
	);
}
