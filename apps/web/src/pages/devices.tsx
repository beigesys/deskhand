import * as React from 'react';
import { Link, useNavigate, useSearch } from '@tanstack/react-router';
import { CheckSquare, Download, FolderInput, Play, Plus, Power, Search, X } from 'lucide-react';
import { api } from '@/api/client';
import { keys, useAction, useCan, useDevices } from '@/api/hooks';
import type { Device, DeviceFilter, Group } from '@/api/types';
import { AsyncCombobox, groupOption, NO_GROUP } from '@/components/ui/async-combobox';
import { LoadMore } from '@/components/ui/load-more';
import { AgentOutdated, AttentionFlag, barColor, StateText, STATUS_LEGEND } from '@/components/devices';
import { RunDialog } from '@/components/jobs';
import { Button } from '@/components/ui/button';
import { Chip, Empty, PageHeader } from '@/components/ui/display';
import { Checkbox, Input, Select } from '@/components/ui/form';
import { Dialog, DialogContent, DialogFooter, Tip } from '@/components/ui/overlay';
import { Table, Td, Th, Tr } from '@/components/ui/table';
import { ago, cn, span } from '@/lib/utils';
import type { DeviceSearch } from '@/router';

const now = () => Math.floor(Date.now() / 1000);

/** Desktop and Shell for an online device; Wake for an offline one. */
function RowActions({ d, onWake }: { d: Device; onWake: (d: Device) => void }) {
	const canRun = useCan('operator');
	if (!canRun) return null;
	if (!d.online) {
		return (
			<Button size="sm" variant="act" onClick={() => onWake(d)}>
				Wake
			</Button>
		);
	}
	return (
		<span className="flex gap-1">
			<Button size="sm" variant="act" asChild>
				<a href={`/d/${d.id}/`} target="_blank" rel="noreferrer">
					Desktop
				</a>
			</Button>
			<Button size="sm" variant="act" asChild>
				<a href={`/d/${d.id}/shell`} target="_blank" rel="noreferrer">
					Shell
				</a>
			</Button>
		</span>
	);
}

/** A device on a phone: status bar, host and user, chips, flag, actions. Long-press selects. */
function DeviceCard({
	d,
	selecting,
	selected,
	onToggle,
	onLongPress,
	onWake,
}: {
	d: Device;
	selecting: boolean;
	selected: boolean;
	onToggle: () => void;
	onLongPress: () => void;
	onWake: (d: Device) => void;
}) {
	const timer = React.useRef<number | undefined>(undefined);
	const pressed = React.useRef(false);
	const navigate = useNavigate();
	const start = () => {
		pressed.current = false;
		timer.current = window.setTimeout(() => {
			pressed.current = true;
			onLongPress();
		}, 500);
	};
	const cancel = () => window.clearTimeout(timer.current);
	return (
		<li
			data-testid="device-card"
			className={cn(
				'status-bar relative grid gap-2 border-b border-line bg-surface py-3 pl-4 pr-3 last:border-b-0',
				selected && 'bg-signal-soft',
			)}
			style={{ ['--bar' as string]: barColor(d) }}
			onPointerDown={start}
			onPointerUp={cancel}
			onPointerLeave={cancel}
			onPointerCancel={cancel}
			onContextMenu={(e) => e.preventDefault()}
			onClick={(e) => {
				if (pressed.current) {
					e.preventDefault();
					pressed.current = false;
					return;
				}
				if ((e.target as HTMLElement).closest('a,button,[role=checkbox]')) return;
				if (selecting) onToggle();
				else navigate({ to: '/devices/$id', params: { id: d.id }, search: { tab: undefined } });
			}}
		>
			<div className="flex items-start gap-3">
				{selecting && (
					<Checkbox checked={selected} onCheckedChange={onToggle} aria-label={`Select ${d.name}`} className="mt-0.5 size-5" />
				)}
				<div className="min-w-0 flex-1">
					<Link
						to="/devices/$id"
						params={{ id: d.id }}
						search={{ tab: undefined }}
						className="block truncate text-[15px] font-medium"
						onClick={(e) => selecting && e.preventDefault()}
					>
						{d.name}
					</Link>
					<div className="truncate text-xs text-muted">{d.logged_in_users.join(', ') || d.hostname}</div>
				</div>
				<StateText d={d} />
			</div>
			<div className="flex flex-wrap gap-1.5">
				<Chip>{d.os}</Chip>
				{d.ip && <Chip>{d.ip}</Chip>}
				<AgentOutdated d={d} />
				{d.group && <span className="text-xs text-muted">{d.group}</span>}
			</div>
			{d.attention.length > 0 && <AttentionFlag d={d} />}
			{!selecting && (
				<div className="flex gap-2 [&>span]:flex [&>span]:flex-1 [&>span]:gap-2 [&_a]:flex-1 [&_button]:flex-1">
					<RowActions d={d} onWake={onWake} />
				</div>
			)}
		</li>
	);
}

export function DevicesPage() {
	const search = useSearch({ from: '/app/devices' });
	const navigate = useNavigate({ from: '/devices' });
	const devices = useDevices({ q: search.q, group: search.group, status: search.status, os: search.os });
	const canRun = useCan('operator');
	const isAdmin = useCan('admin');
	// Selected devices by id, kept across pages and searches.
	const [picked, setPicked] = React.useState<Map<string, Device>>(new Map());
	const selected = React.useMemo(() => new Set(picked.keys()), [picked]);
	const [moveTo, setMoveTo] = React.useState<string | null>(null);
	const [selecting, setSelecting] = React.useState(false);
	const [run, setRun] = React.useState<null | { source?: string }>(null);
	const [moving, setMoving] = React.useState(false);
	const [q, setQ] = React.useState(search.q ?? '');

	const set = (patch: Partial<DeviceSearch>) => navigate({ search: (s) => ({ ...s, ...patch }), replace: true });

	React.useEffect(() => {
		const t = setTimeout(() => {
			if ((search.q ?? '') !== q) set({ q: q || undefined });
		}, 150);
		return () => clearTimeout(t);
		// eslint-disable-next-line react-hooks/exhaustive-deps
	}, [q]);

	// "Every device matching": the bulk actions send the filters, not ids, so it reaches
	// devices that aren't loaded yet. Any change to the filters ends it.
	const filter: DeviceFilter = { q: search.q, group: search.group, status: search.status, os: search.os };
	const filterKey = JSON.stringify(filter);
	const [all, setAll] = React.useState(false);
	React.useEffect(() => setAll(false), [filterKey]);

	const list = devices.items;
	const chosen = [...picked.values()];
	const count = all ? (devices.total ?? 0) : chosen.length;
	const isChosen = (d: Device) => all || selected.has(d.id);
	const loadedChosen = list.filter(isChosen).length;
	const allChecked = list.length > 0 && loadedChosen === list.length ? true : loadedChosen ? 'indeterminate' : false;
	const toggle = (d: Device) => {
		if (all) {
			// Out of "all matching": keep what's loaded, minus this one.
			setAll(false);
			setPicked(new Map(list.filter((x) => x.id !== d.id).map((x) => [x.id, x])));
			return;
		}
		setPicked((m) => {
			const n = new Map(m);
			if (n.has(d.id)) n.delete(d.id);
			else n.set(d.id, d);
			return n;
		});
	};
	const clear = () => {
		setAll(false);
		setPicked(new Map());
	};
	const endSelect = () => {
		setSelecting(false);
		clear();
	};

	const move = useAction(
		(group: number | null) =>
			api.post<{ moved: number }>(
				'/devices/move',
				all ? { group_id: group, filter } : { group_id: group, devices: chosen.map((d) => d.id) },
			),
		{
			invalidate: [keys.devices, keys.groups],
			success: (r) => `Moved ${r.moved} device${r.moved === 1 ? '' : 's'}`,
			onSuccess: () => {
				setMoving(false);
				setMoveTo(null);
			},
		},
	);
	const wake = useAction((d: Device) => api.post('/jobs', { action: 'wake', devices: [d.id] }), {
		invalidate: [keys.jobs],
		success: (_, d) => `Wake sent to ${d.name}`,
	});

	const total = devices.total;
	const bulk = count > 0 && (
		<>
			<span className="text-xs font-medium tabular" data-testid="selected-count">
				{all ? `All ${count} matching selected` : total != null ? `${count} of ${total} selected` : `${count} selected`}
			</span>
			{!all && total != null && total > count && (
				<Button size="sm" variant="ghost" className="text-signal" onClick={() => setAll(true)}>
					Select all {total} matching
				</Button>
			)}
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
			<Button size="sm" variant="ghost" onClick={clear}>
				Clear
			</Button>
		</>
	);

	return (
		<>
			<PageHeader
				title="Devices"
				sub={devices.total != null ? `${devices.total} device${devices.total === 1 ? '' : 's'}${search.q || search.group || search.status || search.os ? ' match' : ''}` : undefined}
				actions={
					<Button asChild variant="secondary" className="max-md:hidden">
						<a href="/api/v1/devices.csv">
							<Download /> Export CSV
						</a>
					</Button>
				}
				primary={
					<>
						<span className="md:hidden">
							{selecting ? (
								<Button variant="secondary" onClick={endSelect}>
									<X /> Done
								</Button>
							) : (
								<Button variant="secondary" onClick={() => setSelecting(true)}>
									<CheckSquare /> Select
								</Button>
							)}
						</span>
						{isAdmin && (
							<Button variant="primary" asChild className="max-md:hidden">
								<Link to="/enroll">
									<Plus /> Enroll device
								</Link>
							</Button>
						)}
					</>
				}
			/>

			<div className="mb-4 flex flex-wrap items-center gap-2">
				<div className="relative min-w-0 flex-[1_1_260px]">
					<Search className="pointer-events-none absolute left-2.5 top-1/2 size-4 -translate-y-1/2 text-subtle" />
					<Input
						type="search"
						value={q}
						onChange={(e) => setQ(e.target.value)}
						placeholder="Search hostname, user, IP, serial…"
						className="pl-8"
						aria-label="Search devices"
					/>
				</div>
				<AsyncCombobox<Group>
					endpoint="/groups"
					toOption={groupOption}
					extra={[{ id: '', label: 'All groups' }, NO_GROUP]}
					value={search.group ?? ''}
					onChange={(v) => set({ group: v || undefined })}
					placeholder="Group: All"
					searchPlaceholder="Search groups…"
					aria-label="Group"
					className="w-auto min-w-40 max-md:flex-1"
				/>
				<Select
					className="w-auto min-w-32 max-md:flex-1"
					value={search.os ?? ''}
					onChange={(v) => set({ os: v || undefined })}
					aria-label="OS"
					prefix="OS"
					options={[
						{ value: '', label: 'Any' },
						{ value: 'windows', label: 'Windows' },
						{ value: 'linux', label: 'Linux' },
						{ value: 'macos', label: 'macOS' },
					]}
				/>
				<Select<NonNullable<DeviceSearch['status']> | ''>
					className="w-auto min-w-36 max-md:flex-1"
					value={search.status ?? ''}
					onChange={(v) => set({ status: v || undefined })}
					aria-label="Status"
					prefix="Status"
					options={[
						{ value: '', label: 'Any' },
						{ value: 'online', label: 'Online' },
						{ value: 'offline', label: 'Offline' },
						{ value: 'attention', label: 'Needs attention' },
					]}
				/>
				{bulk && (
					<div className="ml-auto flex flex-wrap items-center gap-2 rounded border border-signal/50 bg-signal-soft px-2 py-1 max-md:hidden">
						{bulk}
					</div>
				)}
			</div>

			<div className="overflow-hidden rounded border border-line bg-surface">
				{devices.isLoading ? (
					<Empty>Loading…</Empty>
				) : list.length === 0 ? (
					<Empty>
						{search.q || search.group || search.status || search.os ? (
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
					<>
						{/* Phones: cards. */}
						<ul className="md:hidden" aria-label="Devices">
							{list.map((d) => (
								<DeviceCard
									key={d.id}
									d={d}
									selecting={selecting}
									selected={isChosen(d)}
									onToggle={() => toggle(d)}
									onLongPress={() => {
										setSelecting(true);
										if (!all) setPicked((m) => new Map(m).set(d.id, d));
									}}
									onWake={(d) => wake.mutate(d)}
								/>
							))}
						</ul>
						{/* Everything wider: the table. */}
						<Table className="max-md:hidden">
							<thead>
								<tr>
									<Th className="w-8 pl-4">
										<Checkbox
											checked={allChecked}
											onCheckedChange={(v) => {
												if (all) return clear();
												setPicked((m) => {
													const n = new Map(m);
													for (const d of list) {
														if (v) n.set(d.id, d);
														else n.delete(d.id);
													}
													return n;
												});
											}}
											aria-label="Select all shown"
										/>
									</Th>
									<Th>Device</Th>
									<Th>
										<Tip label={STATUS_LEGEND}>
											<span className="cursor-help underline decoration-dotted underline-offset-2">Status</span>
										</Tip>
									</Th>
									<Th>OS</Th>
									<Th className="hidden lg:table-cell">IP</Th>
									<Th className="hidden lg:table-cell">Group</Th>
									<Th>Attention</Th>
									<Th className="hidden xl:table-cell">Last boot</Th>
									<Th />
								</tr>
							</thead>
							<tbody>
								{list.map((d) => (
									<Tr
										key={d.id}
										className={cn('status-bar', isChosen(d) && '[&>td]:bg-signal-soft')}
										style={{ ['--bar' as string]: barColor(d) }}
									>
										<Td className="pl-4">
											<Checkbox checked={isChosen(d)} onCheckedChange={() => toggle(d)} aria-label={`Select ${d.name}`} />
										</Td>
										<Td className="max-w-60 py-1">
											<Link
												to="/devices/$id"
												params={{ id: d.id }}
												search={{ tab: undefined }}
												className="block truncate font-medium hover:text-signal"
											>
												{d.name}
											</Link>
											<small className="-mt-0.5 block truncate text-[11.5px] text-muted">
												{d.logged_in_users.join(', ') || (d.name !== d.hostname ? d.hostname : '—')}
											</small>
										</Td>
										<Td>
											<StateText d={d} />
										</Td>
										<Td>
											<Chip>{d.os}</Chip>
										</Td>
										<Td className="hidden lg:table-cell">{d.ip ? <Chip>{d.ip}</Chip> : <span className="text-muted">—</span>}</Td>
										<Td className="hidden lg:table-cell">{d.group ?? <span className="text-muted">—</span>}</Td>
										<Td>
											<span className="flex items-center gap-2">
												<AttentionFlag d={d} />
												<AgentOutdated d={d} />
											</span>
										</Td>
										<Td className="hidden font-mono text-xs tabular text-muted xl:table-cell" title={d.last_boot ? ago(d.last_boot) : undefined}>
											{d.last_boot ? span(now() - d.last_boot) : '—'}
										</Td>
										<Td className="w-0">
											<RowActions d={d} onWake={(d) => wake.mutate(d)} />
										</Td>
									</Tr>
								))}
							</tbody>
						</Table>
						<LoadMore
							hasNextPage={!!devices.hasNextPage}
							isFetchingNextPage={devices.isFetchingNextPage}
							fetchNextPage={devices.fetchNextPage}
							shown={list.length}
							total={devices.total}
						/>
					</>
				)}
			</div>

			{/* Phones: the bulk bar sits at the bottom while devices are selected. */}
			{bulk && (
				<div className="fixed inset-x-0 bottom-0 z-30 flex flex-wrap items-center gap-2 border-t border-line bg-surface px-3 py-2 md:hidden">
					{bulk}
				</div>
			)}

			<RunDialog
				open={!!run}
				onOpenChange={(v) => !v && setRun(null)}
				devices={all ? undefined : chosen}
				filter={all ? filter : undefined}
				initial={run ?? undefined}
			/>

			<Dialog open={moving} onOpenChange={setMoving}>
				<DialogContent
					title={`Move ${count} device${count === 1 ? '' : 's'}`}
					description={all ? "Every device matching the list's filters" : undefined}
				>
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
							value={moveTo}
							onChange={setMoveTo}
							placeholder="Pick a group…"
							searchPlaceholder="Search groups…"
							aria-label="Group"
						/>
						<DialogFooter>
							<Button variant="ghost" onClick={() => setMoving(false)}>
								Cancel
							</Button>
							<Button type="submit" variant="primary" disabled={move.isPending || !moveTo}>
								Move
							</Button>
						</DialogFooter>
					</form>
				</DialogContent>
			</Dialog>
		</>
	);
}
