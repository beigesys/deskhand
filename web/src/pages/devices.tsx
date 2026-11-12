import * as React from 'react';
import { Link, useNavigate, useSearch } from '@tanstack/react-router';
import { Download, FolderInput, Play, Power, Search } from 'lucide-react';
import { api } from '@/api/client';
import { keys, useAction, useCan, useDevices, useGroups } from '@/api/hooks';
import type { Device } from '@/api/types';
import { AttentionBadges, barColor, OsChip } from '@/components/devices';
import { RunDialog } from '@/components/jobs';
import { Button } from '@/components/ui/button';
import { Chip, Empty, PageHeader } from '@/components/ui/display';
import { Checkbox, Input, Select } from '@/components/ui/form';
import { Dialog, DialogContent, DialogFooter } from '@/components/ui/overlay';
import { Table, Td, Th, Tr } from '@/components/ui/table';
import { ago, cn } from '@/lib/utils';
import type { DeviceSearch } from '@/router';

function osFamily(d: Device): string {
	const p = d.platform.toLowerCase();
	if (p.includes('win')) return 'windows';
	if (p.includes('mac') || p.includes('darwin')) return 'macos';
	return 'linux';
}

export function DevicesPage() {
	const search = useSearch({ from: '/app/devices' });
	const navigate = useNavigate({ from: '/devices' });
	const devices = useDevices();
	const groups = useGroups();
	const canRun = useCan('operator');
	const isAdmin = useCan('admin');
	const [selected, setSelected] = React.useState<Set<string>>(new Set());
	const [run, setRun] = React.useState<null | { source?: string }>(null);
	const [moving, setMoving] = React.useState(false);
	const [q, setQ] = React.useState(search.q ?? '');

	const set = (patch: Partial<DeviceSearch>) => navigate({ search: (s) => ({ ...s, ...patch }), replace: true });

	// Debounced search box -> URL.
	React.useEffect(() => {
		const t = setTimeout(() => {
			if ((search.q ?? '') !== q) set({ q: q || undefined });
		}, 150);
		return () => clearTimeout(t);
		// eslint-disable-next-line react-hooks/exhaustive-deps
	}, [q]);

	const list = React.useMemo(() => {
		const needle = (search.q ?? '').toLowerCase();
		return (devices.data ?? []).filter((d) => {
			if (search.group === 'none' ? d.group_id !== null : search.group && String(d.group_id) !== search.group) return false;
			if (search.status === 'online' && !d.online) return false;
			if (search.status === 'offline' && d.online) return false;
			if (search.status === 'attention' && !d.attention.length) return false;
			if (search.os && osFamily(d) !== search.os) return false;
			if (!needle) return true;
			return [d.name, d.hostname, d.os, d.ip, d.public_ip, d.serial, d.group, d.model, ...d.logged_in_users]
				.filter(Boolean)
				.some((s) => s!.toLowerCase().includes(needle));
		});
	}, [devices.data, search]);

	const chosen = list.filter((d) => selected.has(d.id));
	const allChecked = list.length > 0 && chosen.length === list.length ? true : chosen.length ? 'indeterminate' : false;
	const online = (devices.data ?? []).filter((d) => d.online).length;

	const move = useAction(
		async (group: number | null) => {
			for (const d of chosen) await api.post(`/devices/${d.id}/move`, { group_id: group });
		},
		{ invalidate: [keys.devices, keys.groups], success: 'Moved', onSuccess: () => setMoving(false) },
	);

	return (
		<>
			<PageHeader
				title="Devices"
				sub={
					<>
						{devices.data?.length ?? 0} devices · <span className="text-ok">{online} online</span>
					</>
				}
				actions={
					<Button asChild variant="secondary">
						<a href="/api/v1/devices.csv">
							<Download /> Export CSV
						</a>
					</Button>
				}
			/>

			<div className="mb-3 flex flex-wrap items-center gap-2">
				<div className="relative min-w-56 flex-1 sm:max-w-xs">
					<Search className="pointer-events-none absolute left-2 top-1/2 size-4 -translate-y-1/2 text-subtle" />
					<Input
						value={q}
						onChange={(e) => setQ(e.target.value)}
						placeholder="Search name, IP, serial, user…"
						className="pl-7"
						aria-label="Search devices"
					/>
				</div>
				<Select
					className="w-auto"
					value={search.status ?? ''}
					onChange={(e) => set({ status: (e.target.value || undefined) as DeviceSearch['status'] })}
					aria-label="Status"
				>
					<option value="">Any status</option>
					<option value="online">Online</option>
					<option value="offline">Offline</option>
					<option value="attention">Needs attention</option>
				</Select>
				<Select
					className="w-auto"
					value={search.group ?? ''}
					onChange={(e) => set({ group: e.target.value || undefined })}
					aria-label="Group"
				>
					<option value="">All groups</option>
					<option value="none">No group</option>
					{(groups.data ?? []).map((g) => (
						<option key={g.id} value={g.id}>
							{g.name}
						</option>
					))}
				</Select>
				<Select className="w-auto" value={search.os ?? ''} onChange={(e) => set({ os: e.target.value || undefined })} aria-label="OS">
					<option value="">Any OS</option>
					<option value="windows">Windows</option>
					<option value="linux">Linux</option>
					<option value="macos">macOS</option>
				</Select>
				{chosen.length > 0 && (
					<div className="ml-auto flex flex-wrap items-center gap-2 rounded border border-signal/50 bg-signal-soft px-2 py-1">
						<span className="text-xs font-medium">{chosen.length} selected</span>
						{canRun && (
							<>
								<Button size="sm" variant="primary" onClick={() => setRun({})}>
									<Play /> Run script
								</Button>
								<Button size="sm" variant="danger" onClick={() => setRun({ source: 'action:reboot' })}>
									<Power /> Reboot
								</Button>
							</>
						)}
						{isAdmin && (
							<Button size="sm" onClick={() => setMoving(true)}>
								<FolderInput /> Move
							</Button>
						)}
						<Button size="sm" variant="ghost" onClick={() => setSelected(new Set())}>
							Clear
						</Button>
					</div>
				)}
			</div>

			<div className="rounded border border-line bg-surface">
				{devices.isLoading ? (
					<Empty>Loading…</Empty>
				) : list.length === 0 ? (
					<Empty>
						{devices.data?.length ? (
							'No devices match.'
						) : (
							<>
								No devices yet.{' '}
								{isAdmin && (
									<Link to="/enroll" className="text-signal hover:underline">
										Create an enrollment token
									</Link>
								)}
							</>
						)}
					</Empty>
				) : (
					<Table>
						<thead>
							<tr>
								<Th className="w-8 pl-4">
									<Checkbox
										checked={allChecked}
										onCheckedChange={(v) => setSelected(v ? new Set(list.map((d) => d.id)) : new Set())}
										aria-label="Select all"
									/>
								</Th>
								<Th>Name</Th>
								<Th>Status</Th>
								<Th>OS</Th>
								<Th className="hidden lg:table-cell">IP</Th>
								<Th className="hidden md:table-cell">Group</Th>
								<Th className="hidden xl:table-cell">User</Th>
								<Th className="hidden lg:table-cell">Attention</Th>
							</tr>
						</thead>
						<tbody>
							{list.map((d) => (
								<Tr
									key={d.id}
									className={cn('status-bar', selected.has(d.id) && 'bg-signal-soft')}
									style={{ ['--bar' as string]: barColor(d) }}
								>
									<Td className="pl-4">
										<Checkbox
											checked={selected.has(d.id)}
											onCheckedChange={(v) =>
												setSelected((s) => {
													const n = new Set(s);
													if (v) n.add(d.id);
													else n.delete(d.id);
													return n;
												})
											}
											aria-label={`Select ${d.name}`}
										/>
									</Td>
									<Td className="max-w-64">
										<Link
											to="/devices/$id"
											params={{ id: d.id }}
											search={{ tab: undefined }}
											className="block truncate font-medium hover:text-signal"
										>
											{d.name}
										</Link>
										{d.name !== d.hostname && <div className="truncate text-xs text-muted">{d.hostname}</div>}
									</Td>
									<Td className="text-xs">
										{d.online ? (
											<span className="text-ok">Online</span>
										) : (
											<span className="text-off">Seen {ago(d.last_seen)}</span>
										)}
									</Td>
									<Td>
										<OsChip d={d} />
									</Td>
									<Td className="hidden lg:table-cell">{d.ip ? <Chip>{d.ip}</Chip> : <span className="text-subtle">—</span>}</Td>
									<Td className="hidden text-muted md:table-cell">{d.group ?? '—'}</Td>
									<Td className="hidden text-muted xl:table-cell">{d.logged_in_users.join(', ') || '—'}</Td>
									<Td className="hidden lg:table-cell">
										<AttentionBadges d={d} max={2} />
									</Td>
								</Tr>
							))}
						</tbody>
					</Table>
				)}
			</div>

			<RunDialog open={!!run} onOpenChange={(v) => !v && setRun(null)} devices={chosen} initial={run ?? undefined} />

			<Dialog open={moving} onOpenChange={setMoving}>
				<DialogContent title={`Move ${chosen.length} device${chosen.length === 1 ? '' : 's'}`}>
					<form
						onSubmit={(e) => {
							e.preventDefault();
							const v = new FormData(e.currentTarget).get('group') as string;
							move.mutate(v ? Number(v) : null);
						}}
					>
						<Select name="group" defaultValue="" aria-label="Group">
							<option value="">No group</option>
							{(groups.data ?? []).map((g) => (
								<option key={g.id} value={g.id}>
									{g.name}
								</option>
							))}
						</Select>
						<DialogFooter>
							<Button variant="ghost" onClick={() => setMoving(false)}>
								Cancel
							</Button>
							<Button type="submit" variant="primary" disabled={move.isPending}>
								Move
							</Button>
						</DialogFooter>
					</form>
				</DialogContent>
			</Dialog>
		</>
	);
}
