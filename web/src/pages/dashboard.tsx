import { Link, useNavigate } from '@tanstack/react-router';
import { useDevices, useJobs } from '@/api/hooks';
import { AttentionBadges, barColor, OsChip } from '@/components/devices';
import { Empty, Gauge, PageHeader, Panel, StatusBadge } from '@/components/ui/display';
import { Table, Td, Th, Tr } from '@/components/ui/table';
import { ago, until } from '@/lib/utils';

export function DashboardPage() {
	const devices = useDevices();
	const jobs = useJobs();
	const navigate = useNavigate();
	const list = devices.data ?? [];
	const online = list.filter((d) => d.online).length;
	const attention = list.filter((d) => d.attention.length);
	const failing = (jobs.data ?? []).reduce((n, j) => n + (j.counts?.failed ?? 0) + (j.counts?.timeout ?? 0), 0);
	const running = (jobs.data ?? []).reduce((n, j) => n + (j.counts?.running ?? 0) + (j.counts?.queued ?? 0), 0);
	const recent = [...(jobs.data ?? [])].sort((a, b) => (b.last_fired ?? b.created) - (a.last_fired ?? a.created)).slice(0, 8);

	return (
		<>
			<PageHeader title="Dashboard" sub={`${list.length} devices in the fleet`} />
			<div className="grid grid-cols-2 gap-3 lg:grid-cols-4">
				<Gauge
					label="Online"
					value={online}
					tone="ok"
					sub={`of ${list.length}`}
					onClick={() => navigate({ to: '/devices', search: { status: 'online' } })}
				/>
				<Gauge
					label="Offline"
					value={list.length - online}
					tone="off"
					onClick={() => navigate({ to: '/devices', search: { status: 'offline' } })}
				/>
				<Gauge
					label="Needs attention"
					value={attention.length}
					tone={attention.length ? 'warn' : undefined}
					onClick={() => navigate({ to: '/devices', search: { status: 'attention' } })}
				/>
				<Gauge
					label="Failed runs"
					value={failing}
					tone={failing ? 'bad' : undefined}
					sub={running ? `${running} queued or running` : 'none running'}
					onClick={() => navigate({ to: '/jobs' })}
				/>
			</div>

			<div className="mt-3 grid gap-3 xl:grid-cols-2">
				<Panel title="Needs attention" bodyClassName="p-0">
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
								{attention.slice(0, 12).map((d) => (
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
											<AttentionBadges d={d} />
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
