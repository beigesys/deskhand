import * as React from 'react';
import { useNavigate } from '@tanstack/react-router';
import { useQuery } from '@tanstack/react-query';
import { api } from '@/api/client';
import { keys, useAction } from '@/api/hooks';
import type { Device, Group, JobKind, NewJob, Page, Script } from '@/api/types';
import { AsyncCombobox, deviceOption, groupOption, scriptOption } from '@/components/ui/async-combobox';
import { Button } from '@/components/ui/button';
import { Field, Input, Select } from '@/components/ui/form';
import { Dialog, DialogContent, DialogFooter } from '@/components/ui/overlay';

export const ACTIONS: { value: Exclude<JobKind, 'script'>; label: string }[] = [
	{ value: 'reboot', label: 'Reboot' },
	{ value: 'shutdown', label: 'Shut down' },
	{ value: 'logoff', label: 'Sign out user (Windows)' },
	{ value: 'lock', label: 'Lock (Windows)' },
	{ value: 'wake', label: 'Wake-on-LAN' },
];

/** What the typed confirmation must be for reboot/shutdown: hostname, or the count. */
export function confirmText(devices: Pick<Device, 'hostname'>[]): string {
	return devices.length === 1 ? devices[0].hostname : String(devices.length);
}

function Segmented<T extends string>({
	value,
	onChange,
	options,
	label,
}: {
	value: T;
	onChange: (v: T) => void;
	options: { value: T; label: string }[];
	label: string;
}) {
	return (
		<div role="radiogroup" aria-label={label} className="inline-flex rounded border border-line bg-surface p-0.5">
			{options.map((o) => (
				<button
					key={o.value}
					type="button"
					role="radio"
					aria-checked={value === o.value}
					onClick={() => onChange(o.value)}
					className="h-[calc(var(--dh-control)-6px)] rounded-sm px-3 text-[13px] text-muted aria-checked:bg-chip aria-checked:font-medium aria-checked:text-fg"
				>
					{o.label}
				</button>
			))}
		</div>
	);
}

/**
 * Run a script or power action. With `devices`, it targets exactly those; otherwise the
 * user picks devices (searchable), a group, or everything.
 */
export function RunDialog({
	open,
	onOpenChange,
	devices: fixed,
	initial,
}: {
	open: boolean;
	onOpenChange: (v: boolean) => void;
	devices?: Device[];
	initial?: { source?: string };
}) {
	const navigate = useNavigate();
	const [what, setWhat] = React.useState<'script' | 'action'>('script');
	const [scriptId, setScriptId] = React.useState<string | null>(null);
	const [action, setAction] = React.useState<Exclude<JobKind, 'script'>>('reboot');
	const [target, setTarget] = React.useState<'devices' | 'group' | 'all'>('devices');
	const [picked, setPicked] = React.useState<string[]>([]);
	const [group, setGroup] = React.useState<string | null>(null);
	const [when, setWhen] = React.useState<'now' | 'cron'>('now');
	const [cron, setCron] = React.useState('0 3 * * *');
	const [delay, setDelay] = React.useState('0');
	const [message, setMessage] = React.useState('');
	const [confirm, setConfirm] = React.useState('');
	const [name, setName] = React.useState('');

	React.useEffect(() => {
		if (!open) return;
		const src = initial?.source ?? '';
		if (src.startsWith('action:')) {
			setWhat('action');
			setAction(src.slice(7) as Exclude<JobKind, 'script'>);
		} else {
			setWhat('script');
			setScriptId(src.startsWith('script:') ? src.slice(7) : null);
		}
		setConfirm('');
		setPicked([]);
		setGroup(null);
		setTarget('devices');
		setWhen('now');
	}, [open, initial?.source]);

	const power = what === 'action' && (action === 'reboot' || action === 'shutdown');

	// How many devices it reaches, and for one device its hostname (the confirmation).
	const reach = useQuery({
		queryKey: ['run-reach', fixed?.map((d) => d.id) ?? null, target, picked, group],
		enabled: open && !fixed,
		queryFn: async () => {
			const params =
				target === 'all' ? '' : target === 'group' ? (group ? `group=${group}` : 'ids=') : `ids=${picked.join(',')}`;
			if ((target === 'devices' && !picked.length) || (target === 'group' && !group)) return { total: 0, one: null };
			const p = await api.get<Page<Device>>(`/devices?${params}&limit=1`);
			return { total: p.total ?? 0, one: p.total === 1 ? p.items[0] : null };
		},
	});
	const count = fixed ? fixed.length : (reach.data?.total ?? 0);
	const one = fixed ? (fixed.length === 1 ? fixed[0] : null) : (reach.data?.one ?? null);
	const need = power ? (count === 1 && one ? one.hostname : String(count)) : '';
	const confirmed = !power || confirm.trim().toLowerCase() === need.toLowerCase();
	const ready = (what === 'action' || scriptId) && count > 0 && confirmed;

	const create = useAction((j: NewJob) => api.post<{ id: number }>('/jobs', j), {
		invalidate: [keys.jobs],
		success: (_, j) => (j.cron ? 'Job scheduled' : 'Job started'),
		onSuccess: (r) => {
			onOpenChange(false);
			navigate({ to: '/jobs/$id', params: { id: String(r.id) } });
		},
	});

	function submit(e: React.FormEvent) {
		e.preventDefault();
		const j: NewJob = {};
		if (what === 'script') j.script_id = Number(scriptId);
		else j.action = action;
		if (fixed) j.devices = fixed.map((d) => d.id);
		else if (target === 'devices') j.devices = picked;
		else if (target === 'group') j.group_id = Number(group);
		else j.all = true;
		if (when === 'cron') j.cron = cron;
		if (power) {
			j.delay_s = Number(delay) || 0;
			if (message) j.message = message;
			j.confirm = confirm.trim();
		}
		if (name.trim()) j.name = name.trim();
		create.mutate(j);
	}

	return (
		<Dialog open={open} onOpenChange={onOpenChange}>
			<DialogContent
				title={fixed?.length === 1 ? `Run on ${fixed[0].name}` : 'Run a job'}
				description={fixed && fixed.length > 1 ? `${fixed.length} devices selected` : undefined}
				className="max-w-xl"
			>
				<form onSubmit={submit} className="grid gap-3">
					<Segmented
						label="What to run"
						value={what}
						onChange={setWhat}
						options={[
							{ value: 'script', label: 'Script' },
							{ value: 'action', label: 'Power or session' },
						]}
					/>
					{what === 'script' ? (
						<Field label="Script">
							<AsyncCombobox<Script>
								endpoint="/scripts"
								toOption={scriptOption}
								value={scriptId}
								onChange={(v) => setScriptId(v)}
								placeholder="Pick a script…"
								searchPlaceholder="Search scripts…"
								aria-label="Script"
							/>
						</Field>
					) : (
						<Field label="Action">
							<Select value={action} onChange={(e) => setAction(e.target.value as typeof action)}>
								{ACTIONS.map((a) => (
									<option key={a.value} value={a.value}>
										{a.label}
									</option>
								))}
							</Select>
						</Field>
					)}

					{!fixed && (
						<>
							<Segmented
								label="Target"
								value={target}
								onChange={setTarget}
								options={[
									{ value: 'devices', label: 'Devices' },
									{ value: 'group', label: 'Group' },
									{ value: 'all', label: 'All devices' },
								]}
							/>
							{target === 'devices' && (
								<AsyncCombobox<Device>
									multiple
									endpoint="/devices"
									toOption={deviceOption}
									value={picked}
									onChange={(v) => setPicked(v)}
									placeholder="Pick devices…"
									searchPlaceholder="Search name, IP, serial…"
									aria-label="Devices"
								/>
							)}
							{target === 'group' && (
								<AsyncCombobox<Group>
									endpoint="/groups"
									toOption={groupOption}
									value={group}
									onChange={(v) => setGroup(v)}
									placeholder="Pick a group…"
									searchPlaceholder="Search groups…"
									aria-label="Group"
								/>
							)}
						</>
					)}

					<div className="grid gap-2 sm:grid-cols-[auto_1fr] sm:items-end">
						<Field label="When">
							<Segmented
								label="When"
								value={when}
								onChange={setWhen}
								options={[
									{ value: 'now', label: 'Now' },
									{ value: 'cron', label: 'Schedule' },
								]}
							/>
						</Field>
						{when === 'cron' && (
							<Field label="Cron (UTC)" hint="minute hour day month weekday">
								<Input value={cron} onChange={(e) => setCron(e.target.value)} className="font-mono" required />
							</Field>
						)}
					</div>

					{power && (
						<div className="grid gap-2 sm:grid-cols-[160px_1fr]">
							<Field label="Delay (seconds)">
								<Input type="number" min={0} max={86400} value={delay} onChange={(e) => setDelay(e.target.value)} />
							</Field>
							<Field label="Message to the user">
								<Input value={message} onChange={(e) => setMessage(e.target.value)} placeholder="optional" />
							</Field>
						</div>
					)}

					<Field label="Job name" hint="Optional; defaults to what and where.">
						<Input value={name} onChange={(e) => setName(e.target.value)} />
					</Field>

					{power && count > 0 && (
						<div className="rounded border border-signal/50 bg-signal-soft p-3">
							<Field
								label={`Confirm ${action}`}
								hint={
									<>
										{count === 1 ? 'Type the hostname' : 'Type the number of devices'}: <code className="chip">{need}</code>
									</>
								}
							>
								<Input value={confirm} onChange={(e) => setConfirm(e.target.value)} autoComplete="off" />
							</Field>
						</div>
					)}

					<DialogFooter>
						<span className="mr-auto self-center text-xs text-muted">
							{count} device{count === 1 ? '' : 's'}
						</span>
						<Button variant="ghost" onClick={() => onOpenChange(false)}>
							Cancel
						</Button>
						<Button type="submit" variant={power ? 'danger' : 'primary'} disabled={!ready || create.isPending}>
							{when === 'cron' ? 'Schedule' : power ? `${action === 'reboot' ? 'Reboot' : 'Shut down'} now` : 'Run now'}
						</Button>
					</DialogFooter>
				</form>
			</DialogContent>
		</Dialog>
	);
}
