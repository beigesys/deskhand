import * as React from 'react';
import { useNavigate } from '@tanstack/react-router';
import { api } from '@/api/client';
import { keys, useAction, useDevices, useGroups, useScripts } from '@/api/hooks';
import type { Device, JobKind, NewJob } from '@/api/types';
import { Button } from '@/components/ui/button';
import { Field, Input, Select } from '@/components/ui/form';
import { Dialog, DialogContent, DialogFooter } from '@/components/ui/overlay';
import { Chip } from '@/components/ui/display';

export const ACTIONS: { value: Exclude<JobKind, 'script'>; label: string }[] = [
	{ value: 'reboot', label: 'Reboot' },
	{ value: 'shutdown', label: 'Shut down' },
	{ value: 'logoff', label: 'Sign out user (Windows)' },
	{ value: 'lock', label: 'Lock (Windows)' },
	{ value: 'wake', label: 'Wake-on-LAN' },
];

/** What the typed confirmation must be for reboot/shutdown: hostname, or the count. */
export function confirmText(devices: Device[]): string {
	return devices.length === 1 ? devices[0].hostname : String(devices.length);
}

/**
 * Run a script or power action. With `devices`, it targets exactly those; otherwise the
 * user picks devices, a group or everything.
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
	const scripts = useScripts();
	const groups = useGroups();
	const all = useDevices();
	const navigate = useNavigate();
	const [source, setSource] = React.useState(initial?.source ?? '');
	const [target, setTarget] = React.useState<'devices' | 'group' | 'all'>('devices');
	const [picked, setPicked] = React.useState<string[]>([]);
	const [group, setGroup] = React.useState('');
	const [when, setWhen] = React.useState<'now' | 'cron'>('now');
	const [cron, setCron] = React.useState('0 3 * * *');
	const [delay, setDelay] = React.useState('0');
	const [message, setMessage] = React.useState('');
	const [confirm, setConfirm] = React.useState('');
	const [name, setName] = React.useState('');

	React.useEffect(() => {
		if (open) {
			setSource(initial?.source ?? '');
			setConfirm('');
			setPicked([]);
			setTarget('devices');
			setWhen('now');
		}
	}, [open, initial?.source]);

	const action = source.startsWith('action:') ? (source.slice(7) as Exclude<JobKind, 'script'>) : null;
	const scriptId = source.startsWith('script:') ? Number(source.slice(7)) : null;
	const power = action === 'reboot' || action === 'shutdown';
	const targetDevices: Device[] = fixed
		? fixed
		: target === 'devices'
			? (all.data ?? []).filter((d) => picked.includes(d.id))
			: target === 'group'
				? (all.data ?? []).filter((d) => String(d.group_id) === group)
				: (all.data ?? []);
	const need = power ? confirmText(targetDevices) : '';
	const confirmed = !power || confirm.trim().toLowerCase() === need.toLowerCase();

	const create = useAction((j: NewJob) => api.post<{ id: number }>('/jobs', j), {
		invalidate: [keys.jobs],
		success: 'Job started',
		onSuccess: (r) => {
			onOpenChange(false);
			navigate({ to: '/jobs/$id', params: { id: String(r.id) } });
		},
	});

	function submit(e: React.FormEvent) {
		e.preventDefault();
		const j: NewJob = {};
		if (scriptId) j.script_id = scriptId;
		if (action) j.action = action;
		if (fixed || target === 'devices') j.devices = targetDevices.map((d) => d.id);
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
					<Field label="What">
						<Select value={source} onChange={(e) => setSource(e.target.value)} required>
							<option value="">Pick a script or action…</option>
							<optgroup label="Scripts">
								{(scripts.data ?? []).map((s) => (
									<option key={s.id} value={`script:${s.id}`}>
										{s.name} ({s.shell})
									</option>
								))}
							</optgroup>
							<optgroup label="Power and session">
								{ACTIONS.map((a) => (
									<option key={a.value} value={`action:${a.value}`}>
										{a.label}
									</option>
								))}
							</optgroup>
						</Select>
					</Field>

					{!fixed && (
						<div className="grid gap-2 sm:grid-cols-[160px_1fr]">
							<Field label="Target">
								<Select value={target} onChange={(e) => setTarget(e.target.value as typeof target)}>
									<option value="devices">Pick devices</option>
									<option value="group">A group</option>
									<option value="all">All devices</option>
								</Select>
							</Field>
							{target === 'group' && (
								<Field label="Group">
									<Select value={group} onChange={(e) => setGroup(e.target.value)} required>
										<option value="">Pick a group…</option>
										{(groups.data ?? []).map((g) => (
											<option key={g.id} value={g.id}>
												{g.name} ({g.devices})
											</option>
										))}
									</Select>
								</Field>
							)}
						</div>
					)}
					{!fixed && target === 'devices' && (
						<div className="max-h-48 overflow-y-auto rounded border border-line">
							{(all.data ?? []).map((d) => (
								<label key={d.id} className="flex h-8 cursor-pointer items-center gap-2 border-b border-line px-2 last:border-0 hover:bg-surface-2">
									<input
										type="checkbox"
										className="accent-[var(--dh-signal)]"
										checked={picked.includes(d.id)}
										onChange={(e) =>
											setPicked((p) => (e.target.checked ? [...p, d.id] : p.filter((x) => x !== d.id)))
										}
									/>
									<span className={d.online ? 'text-ok' : 'text-off'}>●</span>
									<span className="flex-1 truncate">{d.name}</span>
									<Chip>{d.os}</Chip>
								</label>
							))}
						</div>
					)}

					<div className="grid gap-2 sm:grid-cols-[160px_1fr]">
						<Field label="When">
							<Select value={when} onChange={(e) => setWhen(e.target.value as typeof when)}>
								<option value="now">Now</option>
								<option value="cron">On a schedule</option>
							</Select>
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

					{power && targetDevices.length > 0 && (
						<div className="rounded border border-signal/50 bg-signal-soft p-3">
							<Field
								label={`Confirm ${action}`}
								hint={
									<>
										{targetDevices.length === 1 ? 'Type the hostname' : 'Type the number of devices'}:{' '}
										<code className="chip">{need}</code>
									</>
								}
							>
								<Input value={confirm} onChange={(e) => setConfirm(e.target.value)} autoComplete="off" />
							</Field>
						</div>
					)}

					<DialogFooter>
						<span className="mr-auto self-center text-xs text-muted">
							{targetDevices.length} device{targetDevices.length === 1 ? '' : 's'}
						</span>
						<Button variant="ghost" onClick={() => onOpenChange(false)}>
							Cancel
						</Button>
						<Button
							type="submit"
							variant={power ? 'danger' : 'primary'}
							disabled={!source || !targetDevices.length || !confirmed || create.isPending}
						>
							{when === 'cron' ? 'Schedule' : power ? `${action === 'reboot' ? 'Reboot' : 'Shut down'} now` : 'Run now'}
						</Button>
					</DialogFooter>
				</form>
			</DialogContent>
		</Dialog>
	);
}
