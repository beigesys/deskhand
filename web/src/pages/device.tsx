import * as React from 'react';
import { Link, useNavigate, useParams, useSearch } from '@tanstack/react-router';
import {
	ChevronDown,
	Lock,
	LogOut,
	Monitor,
	MoreHorizontal,
	Pencil,
	Play,
	Power,
	PowerOff,
	RefreshCw,
	SquareTerminal,
	Trash2,
	Zap,
	FolderInput,
} from 'lucide-react';
import { api } from '@/api/client';
import { keys, useAction, useCan, useDevice } from '@/api/hooks';
import type { Device, DeviceDetail, Group, Inventory, JobKind } from '@/api/types';
import { AsyncCombobox, groupOption, NO_GROUP } from '@/components/ui/async-combobox';
import { AttentionBadges, barColor } from '@/components/devices';
import { confirmText, RunDialog } from '@/components/jobs';
import { Button } from '@/components/ui/button';
import { Confirm } from '@/components/ui/confirm';
import { Badge, Chip, Empty, KV, Meter, Panel, StatusBadge } from '@/components/ui/display';
import { Field, Input } from '@/components/ui/form';
import {
	Dialog,
	DialogContent,
	DialogFooter,
	Menu,
	MenuContent,
	MenuItem,
	MenuLabel,
	MenuSeparator,
	MenuTrigger,
	Tabs,
	TabsContent,
	TabsList,
	TabsTrigger,
} from '@/components/ui/overlay';
import { Table, Td, Th, Tr } from '@/components/ui/table';
import { ago, bytes, datetime, duration, span } from '@/lib/utils';

const yesNo = (v: boolean | null | undefined, yes = 'On', no = 'Off') =>
	v == null ? <span className="text-subtle">Unknown</span> : v ? <Badge tone="ok">{yes}</Badge> : <Badge tone="bad">{no}</Badge>;

function Overview({ d, inv, at }: { d: Device; inv: Inventory | null; at: number | null }) {
	return (
		<div className="grid gap-3 lg:grid-cols-2">
			<Panel title="Device">
				<KV
					rows={[
						['Name', d.name],
						['Hostname', <Chip key="h">{d.hostname}</Chip>],
						['Status', d.online ? <span className="text-ok">Online since {datetime(d.online_since)}</span> : <span className="text-off">Last seen {ago(d.last_seen)}</span>],
						['Group', d.group ?? '—'],
						['Device ID', <Chip key="i">{d.id}</Chip>],
						['Signed in', d.logged_in_users.join(', ') || '—'],
						['Inventory', at ? `collected ${ago(at)}` : 'not yet'],
					]}
				/>
			</Panel>
			<Panel title="System">
				{inv ? (
					<KV
						rows={[
							['OS', <Chip key="o">{[inv.os.name, inv.os.version].filter(Boolean).join(' ') || '—'}</Chip>],
							['Build / kernel', [inv.os.build, inv.os.kernel].filter(Boolean).join(' · ') || '—'],
							['Domain', inv.domain],
							['Last boot', inv.last_boot ? `${datetime(inv.last_boot)} (${ago(inv.last_boot)})` : '—'],
							['Uptime', inv.uptime_secs != null ? span(inv.uptime_secs) : '—'],
							['Model', [inv.hardware.manufacturer, inv.hardware.model].filter(Boolean).join(' ') || '—'],
							['Serial', inv.hardware.serial ? <Chip key="s">{inv.hardware.serial}</Chip> : '—'],
							['Public IP', inv.public_ip ? <Chip key="p">{inv.public_ip}</Chip> : '—'],
						]}
					/>
				) : (
					<Empty>No inventory yet.</Empty>
				)}
			</Panel>
		</div>
	);
}

function Hardware({ inv }: { inv: Inventory }) {
	return (
		<div className="grid gap-3 lg:grid-cols-2">
			<Panel title="Processor and memory">
				<KV
					rows={[
						['CPU', inv.cpu.model],
						['Cores / threads', `${inv.cpu.cores ?? '—'} / ${inv.cpu.threads ?? '—'}`],
						['Memory', inv.memory.total ? `${bytes(inv.memory.used)} of ${bytes(inv.memory.total)}` : '—'],
						['Manufacturer', inv.hardware.manufacturer],
						['Model', inv.hardware.model],
						['Serial', inv.hardware.serial ? <Chip key="s">{inv.hardware.serial}</Chip> : '—'],
					]}
				/>
				{inv.memory.total && inv.memory.used != null ? (
					<div className="mt-3">
						<Meter used={inv.memory.used} total={inv.memory.total} />
					</div>
				) : null}
			</Panel>
			<Panel title="Disks" bodyClassName="p-0">
				<Table>
					<thead>
						<tr>
							<Th>Mount</Th>
							<Th>FS</Th>
							<Th>Free</Th>
							<Th>Size</Th>
							<Th className="w-32">Used</Th>
						</tr>
					</thead>
					<tbody>
						{inv.disks.map((d) => (
							<Tr key={d.mount}>
								<Td>
									<Chip>{d.mount}</Chip>
								</Td>
								<Td className="text-muted">{d.fs ?? '—'}</Td>
								<Td className="font-mono text-xs tabular">{bytes(d.free)}</Td>
								<Td className="font-mono text-xs tabular">{bytes(d.total)}</Td>
								<Td>
									<Meter used={d.total - d.free} total={d.total} />
								</Td>
							</Tr>
						))}
					</tbody>
				</Table>
			</Panel>
		</div>
	);
}

function Network({ inv }: { inv: Inventory }) {
	return (
		<Panel title="Interfaces" bodyClassName="p-0">
			<Table>
				<thead>
					<tr>
						<Th>Name</Th>
						<Th>MAC</Th>
						<Th>Addresses</Th>
					</tr>
				</thead>
				<tbody>
					{inv.network.map((n) => (
						<Tr key={n.name}>
							<Td className="font-medium">{n.name}</Td>
							<Td>{n.mac ? <Chip>{n.mac}</Chip> : '—'}</Td>
							<Td className="whitespace-normal">
								<span className="flex flex-wrap gap-1 py-1">
									{n.ips.map((ip) => (
										<Chip key={ip}>{ip}</Chip>
									))}
								</span>
							</Td>
						</Tr>
					))}
				</tbody>
			</Table>
		</Panel>
	);
}

function Software({ inv }: { inv: Inventory }) {
	const [q, setQ] = React.useState('');
	const rows = inv.software.filter((s) =>
		[s.name, s.publisher, s.version].some((x) => x?.toLowerCase().includes(q.toLowerCase())),
	);
	return (
		<Panel
			title={`Installed software (${inv.software.length})`}
			actions={<Input value={q} onChange={(e) => setQ(e.target.value)} placeholder="Filter…" className="h-7 w-48" aria-label="Filter software" />}
			bodyClassName="p-0 max-h-[60vh] overflow-y-auto"
		>
			<Table>
				<thead>
					<tr>
						<Th>Name</Th>
						<Th>Version</Th>
						<Th className="hidden md:table-cell">Publisher</Th>
						<Th className="hidden md:table-cell">Installed</Th>
					</tr>
				</thead>
				<tbody>
					{rows.slice(0, 1000).map((s, i) => (
						<Tr key={`${s.name}-${i}`}>
							<Td className="max-w-md truncate">{s.name}</Td>
							<Td>{s.version ? <Chip>{s.version}</Chip> : '—'}</Td>
							<Td className="hidden max-w-xs truncate text-muted md:table-cell">{s.publisher ?? '—'}</Td>
							<Td className="hidden text-muted md:table-cell">{s.install_date ?? '—'}</Td>
						</Tr>
					))}
				</tbody>
			</Table>
			{rows.length > 1000 && <Empty>Showing 1000 of {rows.length}. Narrow the filter.</Empty>}
		</Panel>
	);
}

function Security({ inv }: { inv: Inventory }) {
	const s = inv.security;
	return (
		<div className="grid gap-3 lg:grid-cols-2">
			<Panel title="Protection">
				<KV
					rows={[
						['Pending reboot', s.pending_reboot == null ? '—' : s.pending_reboot ? <Badge tone="warn">Yes</Badge> : <Badge tone="ok">No</Badge>],
						['Antivirus', s.antivirus ? s.antivirus.name : '—'],
						['AV enabled', yesNo(s.antivirus?.enabled)],
						['Real-time', yesNo(s.antivirus?.realtime)],
						[
							'Signatures',
							s.antivirus?.signature_age_days != null ? `${s.antivirus.signature_age_days} days old` : '—',
						],
					]}
				/>
			</Panel>
			<Panel title="Firewall and encryption">
				<KV
					rows={[
						...s.firewall.map((f): [string, React.ReactNode] => [`Firewall · ${f.profile}`, yesNo(f.enabled)]),
						...(s.encryption === null
							? [['Encryption', <span key="u" className="text-subtle">Unknown (needs admin)</span>] as [string, React.ReactNode]]
							: s.encryption.length === 0
								? [['Encryption', <Badge key="n" tone="warn">None found</Badge>] as [string, React.ReactNode]]
								: s.encryption.map((e): [string, React.ReactNode] => [
										`Volume ${e.volume}`,
										<Badge key={e.volume} tone={e.status === 'off' ? 'bad' : 'ok'}>
											{e.status}
										</Badge>,
									])),
					]}
				/>
			</Panel>
		</div>
	);
}

function Runs({ data }: { data: DeviceDetail }) {
	if (!data.runs.length) return <Empty>No jobs have run on this device.</Empty>;
	return (
		<Panel title="Recent runs" bodyClassName="p-0">
			<Table>
				<thead>
					<tr>
						<Th>Job</Th>
						<Th>Status</Th>
						<Th>Exit</Th>
						<Th>Duration</Th>
						<Th>Queued</Th>
					</tr>
				</thead>
				<tbody>
					{data.runs.map((r) => (
						<Tr key={r.id}>
							<Td>
								<Link to="/jobs/$id" params={{ id: String(r.job_id) }} className="font-medium hover:text-signal">
									{r.job_name}
								</Link>
							</Td>
							<Td>
								<StatusBadge status={r.status} />
							</Td>
							<Td className="font-mono text-xs">{r.exit_code ?? '—'}</Td>
							<Td className="font-mono text-xs">{duration(r.started, r.finished)}</Td>
							<Td className="text-muted">{ago(r.queued)}</Td>
						</Tr>
					))}
				</tbody>
			</Table>
		</Panel>
	);
}

export function DevicePage() {
	const { id } = useParams({ from: '/app/devices/$id' });
	const { tab } = useSearch({ from: '/app/devices/$id' });
	const navigate = useNavigate();
	const q = useDevice(id);
	const canRun = useCan('operator');
	const isAdmin = useCan('admin');
	const [run, setRun] = React.useState(false);
	const [power, setPower] = React.useState<Exclude<JobKind, 'script'> | null>(null);
	const [renaming, setRenaming] = React.useState(false);
	const [moving, setMoving] = React.useState(false);
	const [moveTo, setMoveTo] = React.useState<string | null>(null);
	const [deleting, setDeleting] = React.useState(false);

	const refresh = useAction(() => api.post(`/devices/${id}/refresh`), { success: 'Inventory requested' });
	const powerAction = useAction(
		(v: { action: string; confirm?: string }) =>
			api.post<{ id: number }>('/jobs', { action: v.action, devices: [id], confirm: v.confirm }),
		{
			invalidate: [keys.jobs, keys.device(id)],
			success: (_, v) => `${v.action} sent`,
			onSuccess: () => setPower(null),
		},
	);
	const rename = useAction((name: string) => api.post(`/devices/${id}/rename`, { name }), {
		invalidate: [keys.devices, keys.device(id)],
		success: 'Renamed',
		onSuccess: () => setRenaming(false),
	});
	const move = useAction((group_id: number | null) => api.post(`/devices/${id}/move`, { group_id }), {
		invalidate: [keys.devices, keys.device(id), keys.groups],
		success: 'Moved',
		onSuccess: () => setMoving(false),
	});
	const remove = useAction(() => api.del(`/devices/${id}`), {
		invalidate: [keys.devices],
		success: 'Device removed',
		onSuccess: () => navigate({ to: '/devices' }),
	});

	if (q.isLoading) return <Empty>Loading…</Empty>;
	if (!q.data) return <Empty>That device doesn't exist.</Empty>;
	const { device: d, inventory: inv } = q.data;
	const typed = power === 'reboot' || power === 'shutdown' ? confirmText([d]) : undefined;

	return (
		<>
			<div className="status-bar mb-4 rounded border border-line bg-surface p-4 pl-5" style={{ ['--bar' as string]: barColor(d) }}>
				<div className="flex flex-wrap items-start justify-between gap-3">
					<div className="min-w-0">
						<div className="label-caps">
							<Link to="/devices" search={{}} className="hover:text-fg">
								Devices
							</Link>{' '}
							/ {d.group ?? 'No group'}
						</div>
						<h1 className="mt-1 truncate text-xl font-semibold">{d.name}</h1>
						<div className="mt-1.5 flex flex-wrap items-center gap-1.5">
							{d.online ? <Badge tone="ok">Online</Badge> : <Badge tone="off">Offline · {ago(d.last_seen)}</Badge>}
							<Chip>{d.os}</Chip>
							{d.ip && <Chip>{d.ip}</Chip>}
							{d.serial && <Chip title="Serial">{d.serial}</Chip>}
							<AttentionBadges d={d} max={5} />
						</div>
					</div>
					<div className="flex flex-wrap gap-2">
						{canRun && (
							<>
								<Button variant="dark" asChild disabled={!d.online}>
									<a href={`/d/${d.id}/`} target="_blank" rel="noreferrer" aria-disabled={!d.online}>
										<Monitor /> Desktop
									</a>
								</Button>
								<Button variant="dark" asChild>
									<a href={`/d/${d.id}/shell`} target="_blank" rel="noreferrer">
										<SquareTerminal /> Shell
									</a>
								</Button>
								<Button variant="primary" onClick={() => setRun(true)}>
									<Play /> Run script
								</Button>
								<Menu>
									<MenuTrigger asChild>
										<Button>
											<Power /> Power <ChevronDown />
										</Button>
									</MenuTrigger>
									<MenuContent>
										<MenuItem onSelect={() => setPower('reboot')} danger>
											<Power /> Reboot…
										</MenuItem>
										<MenuItem onSelect={() => setPower('shutdown')} danger>
											<PowerOff /> Shut down…
										</MenuItem>
										<MenuSeparator />
										<MenuItem onSelect={() => setPower('logoff')}>
											<LogOut /> Sign out user
										</MenuItem>
										<MenuItem onSelect={() => setPower('lock')}>
											<Lock /> Lock
										</MenuItem>
										<MenuItem onSelect={() => setPower('wake')} disabled={d.online}>
											<Zap /> Wake-on-LAN
										</MenuItem>
									</MenuContent>
								</Menu>
							</>
						)}
						<Menu>
							<MenuTrigger asChild>
								<Button size="icon" aria-label="More">
									<MoreHorizontal />
								</Button>
							</MenuTrigger>
							<MenuContent>
								{canRun && (
									<MenuItem onSelect={() => refresh.mutate(undefined)} disabled={!d.online}>
										<RefreshCw /> Refresh inventory
									</MenuItem>
								)}
								{isAdmin && (
									<>
										<MenuLabel>Manage</MenuLabel>
										<MenuItem onSelect={() => setRenaming(true)}>
											<Pencil /> Rename…
										</MenuItem>
										<MenuItem onSelect={() => setMoving(true)}>
											<FolderInput /> Move to group…
										</MenuItem>
										<MenuSeparator />
										<MenuItem onSelect={() => setDeleting(true)} danger>
											<Trash2 /> Remove device…
										</MenuItem>
									</>
								)}
								{!canRun && !isAdmin && <MenuItem disabled>No actions for your role</MenuItem>}
							</MenuContent>
						</Menu>
					</div>
				</div>
			</div>

			<Tabs value={tab ?? 'overview'} onValueChange={(t) => navigate({ to: '.', search: { tab: t === 'overview' ? undefined : t }, replace: true })}>
				<TabsList>
					<TabsTrigger value="overview">Overview</TabsTrigger>
					<TabsTrigger value="hardware">Hardware</TabsTrigger>
					<TabsTrigger value="network">Network</TabsTrigger>
					<TabsTrigger value="software">Software</TabsTrigger>
					<TabsTrigger value="security">Security</TabsTrigger>
					<TabsTrigger value="jobs">Jobs</TabsTrigger>
				</TabsList>
				<div className="pt-3">
					<TabsContent value="overview">
						<Overview d={d} inv={inv} at={q.data.inventory_at} />
					</TabsContent>
					{(['hardware', 'network', 'software', 'security'] as const).map((t) => (
						<TabsContent key={t} value={t}>
							{!inv ? (
								<Empty>No inventory yet.</Empty>
							) : t === 'hardware' ? (
								<Hardware inv={inv} />
							) : t === 'network' ? (
								<Network inv={inv} />
							) : t === 'software' ? (
								<Software inv={inv} />
							) : (
								<Security inv={inv} />
							)}
						</TabsContent>
					))}
					<TabsContent value="jobs">
						<Runs data={q.data} />
					</TabsContent>
				</div>
			</Tabs>

			<RunDialog open={run} onOpenChange={setRun} devices={[d]} />

			<Confirm
				open={!!power}
				onOpenChange={(v) => !v && setPower(null)}
				title={
					{
						reboot: `Reboot ${d.name}?`,
						shutdown: `Shut down ${d.name}?`,
						logoff: `Sign out the user on ${d.name}?`,
						lock: `Lock ${d.name}?`,
						wake: `Wake ${d.name}?`,
					}[power ?? 'reboot']
				}
				body={power === 'wake' ? 'An online device on the same network sends the magic packet.' : 'Runs as a job; you can follow it on the Jobs page.'}
				action={{ reboot: 'Reboot', shutdown: 'Shut down', logoff: 'Sign out', lock: 'Lock', wake: 'Wake' }[power ?? 'reboot']}
				danger={power === 'reboot' || power === 'shutdown'}
				typed={typed}
				typedLabel="Hostname"
				busy={powerAction.isPending}
				onConfirm={(t) => power && powerAction.mutate({ action: power, confirm: typed ? t : undefined })}
			/>

			<Dialog open={renaming} onOpenChange={setRenaming}>
				<DialogContent title="Rename device">
					<form
						onSubmit={(e) => {
							e.preventDefault();
							rename.mutate(String(new FormData(e.currentTarget).get('name')));
						}}
					>
						<Field label="Name">
							<Input name="name" defaultValue={d.name} autoFocus required />
						</Field>
						<DialogFooter>
							<Button variant="ghost" onClick={() => setRenaming(false)}>
								Cancel
							</Button>
							<Button type="submit" variant="primary">
								Save
							</Button>
						</DialogFooter>
					</form>
				</DialogContent>
			</Dialog>

			<Dialog open={moving} onOpenChange={setMoving}>
				<DialogContent title="Move to group">
					<form
						onSubmit={(e) => {
							e.preventDefault();
							move.mutate(moveTo && moveTo !== 'none' ? Number(moveTo) : null);
						}}
					>
						<AsyncCombobox<Group>
							endpoint="/groups"
							toOption={groupOption}
							extra={[NO_GROUP]}
							value={moveTo ?? (d.group_id != null ? String(d.group_id) : 'none')}
							onChange={setMoveTo}
							placeholder="Pick a group…"
							searchPlaceholder="Search groups…"
							aria-label="Group"
						/>
						<DialogFooter>
							<Button variant="ghost" onClick={() => setMoving(false)}>
								Cancel
							</Button>
							<Button type="submit" variant="primary">
								Move
							</Button>
						</DialogFooter>
					</form>
				</DialogContent>
			</Dialog>

			<Confirm
				open={deleting}
				onOpenChange={setDeleting}
				title={`Remove ${d.name}?`}
				body="Its agent will need a new enrollment token to come back."
				action="Remove"
				danger
				typed={d.name}
				typedLabel="Device name"
				busy={remove.isPending}
				onConfirm={() => remove.mutate(undefined)}
			/>
		</>
	);
}
