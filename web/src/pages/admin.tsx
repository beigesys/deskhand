import * as React from 'react';
import { Link } from '@tanstack/react-router';
import { Check, Copy, Plus, Trash2, Unlink } from 'lucide-react';
import { api } from '@/api/client';
import { keys, useAction, useAudit, useCan, useGroups, useMe, useTokens, useUsers } from '@/api/hooks';
import type { AuditPage as AuditPageData, AuditRow, NewEnrollToken, Role } from '@/api/types';
import { Button } from '@/components/ui/button';
import { Confirm } from '@/components/ui/confirm';
import { Badge, Empty, PageHeader, Panel } from '@/components/ui/display';
import { Field, Input, Select } from '@/components/ui/form';
import { Table, Td, Th, Tr } from '@/components/ui/table';
import { ago, datetime, until } from '@/lib/utils';

function AdminOnly({ children }: { children: React.ReactNode }) {
	const ok = useCan('admin');
	return ok ? <>{children}</> : <Empty>Only admins can see this page.</Empty>;
}

// --- groups ---

export function GroupsPage() {
	const groups = useGroups();
	const [deleting, setDeleting] = React.useState<{ id: number; name: string } | null>(null);
	const add = useAction((name: string) => api.post('/groups', { name }), { invalidate: [keys.groups], success: 'Group added' });
	const del = useAction((id: number) => api.del(`/groups/${id}`), {
		invalidate: [keys.groups, keys.devices],
		success: 'Group deleted',
		onSuccess: () => setDeleting(null),
	});
	return (
		<AdminOnly>
			<PageHeader title="Groups" sub="Group devices by site, team or role. Jobs and enrollment tokens can target a group." />
			<div className="grid gap-3 lg:grid-cols-[1fr_320px]">
				<div className="rounded border border-line bg-surface">
					{!groups.data?.length ? (
						<Empty>No groups yet.</Empty>
					) : (
						<Table>
							<thead>
								<tr>
									<Th>Name</Th>
									<Th>Devices</Th>
									<Th className="w-10" />
								</tr>
							</thead>
							<tbody>
								{groups.data.map((g) => (
									<Tr key={g.id}>
										<Td>
											<Link to="/devices" search={{ group: String(g.id) }} className="font-medium hover:text-signal">
												{g.name}
											</Link>
										</Td>
										<Td className="font-mono text-xs tabular">{g.devices}</Td>
										<Td>
											<Button size="icon" variant="ghost" aria-label={`Delete ${g.name}`} onClick={() => setDeleting(g)}>
												<Trash2 />
											</Button>
										</Td>
									</Tr>
								))}
							</tbody>
						</Table>
					)}
				</div>
				<Panel title="Add a group">
					<form
						className="grid gap-2"
						onSubmit={(e) => {
							e.preventDefault();
							const f = e.currentTarget;
							add.mutate(String(new FormData(f).get('name')), { onSuccess: () => f.reset() });
						}}
					>
						<Field label="Name">
							<Input name="name" required maxLength={100} />
						</Field>
						<Button type="submit" variant="primary">
							<Plus /> Add group
						</Button>
					</form>
				</Panel>
			</div>
			<Confirm
				open={!!deleting}
				onOpenChange={(v) => !v && setDeleting(null)}
				title={`Delete ${deleting?.name}?`}
				body="Its devices stay, with no group."
				action="Delete"
				danger
				onConfirm={() => deleting && del.mutate(deleting.id)}
			/>
		</AdminOnly>
	);
}

// --- enrollment ---

function CopyBlock({ label, text }: { label: string; text: string }) {
	const [done, setDone] = React.useState(false);
	return (
		<div className="grid gap-1">
			<div className="flex items-center justify-between">
				<span className="label-caps">{label}</span>
				<Button
					size="sm"
					variant="ghost"
					onClick={async () => {
						await navigator.clipboard?.writeText(text).catch(() => {});
						setDone(true);
						setTimeout(() => setDone(false), 1500);
					}}
				>
					{done ? <Check /> : <Copy />} {done ? 'Copied' : 'Copy'}
				</Button>
			</div>
			<pre className="overflow-x-auto whitespace-pre-wrap break-all rounded border border-navy-800 bg-navy-950 p-3 font-mono text-xs text-[#d7e0ec]">
				{text}
			</pre>
		</div>
	);
}

export function EnrollPage() {
	const tokens = useTokens();
	const groups = useGroups();
	const [created, setCreated] = React.useState<NewEnrollToken | null>(null);
	const add = useAction(
		(b: { group_id: number | null; hours: number; max_uses: number }) => api.post<NewEnrollToken>('/enroll-tokens', b),
		{ invalidate: [keys.tokens], success: 'Token created', onSuccess: (t) => setCreated(t) },
	);
	const revoke = useAction((id: number) => api.del(`/enroll-tokens/${id}`), { invalidate: [keys.tokens], success: 'Revoked' });
	return (
		<AdminOnly>
			<PageHeader title="Enrollment" sub="Tokens let new machines join. Each machine trades one for its own secret." />
			<div className="grid gap-3 lg:grid-cols-[360px_1fr]">
				<Panel title="Create a token">
					<form
						className="grid gap-3"
						onSubmit={(e) => {
							e.preventDefault();
							const f = new FormData(e.currentTarget);
							const g = String(f.get('group'));
							add.mutate({ group_id: g ? Number(g) : null, hours: Number(f.get('hours')), max_uses: Number(f.get('max_uses')) });
						}}
					>
						<Field label="Group">
							<Select name="group" defaultValue="">
								<option value="">No group</option>
								{(groups.data ?? []).map((g) => (
									<option key={g.id} value={g.id}>
										{g.name}
									</option>
								))}
							</Select>
						</Field>
						<Field label="Expires in">
							<Select name="hours" defaultValue="24">
								<option value="1">1 hour</option>
								<option value="24">1 day</option>
								<option value="168">7 days</option>
								<option value="720">30 days</option>
							</Select>
						</Field>
						<Field label="Max uses">
							<Input name="max_uses" type="number" min={1} max={100000} defaultValue={1} required />
						</Field>
						<Button type="submit" variant="primary" disabled={add.isPending}>
							<Plus /> Create token
						</Button>
					</form>
				</Panel>
				<div className="grid content-start gap-3">
					{created && (
						<Panel
							title="New token: shown once"
							className="border-signal/60"
							actions={
								<Button size="sm" variant="ghost" onClick={() => setCreated(null)}>
									Done
								</Button>
							}
						>
							<div className="grid gap-3">
								<CopyBlock label="Token" text={created.token} />
								<CopyBlock label="Linux (as root)" text={created.linux} />
								<CopyBlock label="Windows (PowerShell as Administrator)" text={created.windows} />
							</div>
						</Panel>
					)}
					<Panel title="Active tokens" bodyClassName="p-0">
						{!tokens.data?.length ? (
							<Empty>None.</Empty>
						) : (
							<Table>
								<thead>
									<tr>
										<Th>Group</Th>
										<Th>Used</Th>
										<Th>Expires</Th>
										<Th className="w-20" />
									</tr>
								</thead>
								<tbody>
									{tokens.data.map((t) => (
										<Tr key={t.id}>
											<Td>{t.group ?? <span className="text-muted">No group</span>}</Td>
											<Td className="font-mono text-xs tabular">
												{t.uses} / {t.max_uses}
											</Td>
											<Td className="text-muted">{until(t.expires)}</Td>
											<Td>
												<Button size="sm" variant="danger" onClick={() => revoke.mutate(t.id)}>
													Revoke
												</Button>
											</Td>
										</Tr>
									))}
								</tbody>
							</Table>
						)}
					</Panel>
				</div>
			</div>
		</AdminOnly>
	);
}

// --- users ---

const ROLES: Role[] = ['viewer', 'operator', 'admin'];

function EmailCell({ id, email }: { id: number; email: string | null }) {
	const [v, setV] = React.useState(email ?? '');
	React.useEffect(() => setV(email ?? ''), [email]);
	const save = useAction((e: string) => api.post(`/users/${id}/email`, { email: e || null }), {
		invalidate: [keys.users],
		success: 'Email saved',
	});
	return (
		<form
			className="flex gap-1"
			onSubmit={(e) => {
				e.preventDefault();
				save.mutate(v);
			}}
		>
			<Input value={v} onChange={(e) => setV(e.target.value)} type="email" placeholder="email" className="h-7 w-52" aria-label="Email" />
			{v !== (email ?? '') && (
				<Button size="sm" type="submit" variant="primary">
					Save
				</Button>
			)}
		</form>
	);
}

export function UsersPage() {
	const users = useUsers();
	const me = useMe();
	const [deleting, setDeleting] = React.useState<{ id: number; username: string } | null>(null);
	const add = useAction(
		(b: { username: string; email: string; password: string; role: string }) =>
			api.post('/users', { ...b, email: b.email || null, password: b.password || null }),
		{ invalidate: [keys.users], success: 'User added' },
	);
	const role = useAction((v: { id: number; role: Role }) => api.post(`/users/${v.id}/role`, { role: v.role }), {
		invalidate: [keys.users],
		success: 'Role changed',
	});
	const unlink = useAction((v: { id: number; provider: string }) => api.del(`/users/${v.id}/identities/${v.provider}`), {
		invalidate: [keys.users],
		success: 'Sign-in unlinked',
	});
	const del = useAction((id: number) => api.del(`/users/${id}`), {
		invalidate: [keys.users],
		success: 'User deleted',
		onSuccess: () => setDeleting(null),
	});
	const sso = users.data?.sso_enabled ?? false;
	return (
		<AdminOnly>
			<PageHeader title="Users" sub="Viewers see devices. Operators can also open desktops and shells and run jobs. Admins manage everything." />
			<div className="grid gap-3 xl:grid-cols-[1fr_320px]">
				<div className="rounded border border-line bg-surface">
					<Table>
						<thead>
							<tr>
								<Th>User</Th>
								<Th>Role</Th>
								<Th>Email</Th>
								<Th>Signs in with</Th>
								<Th className="hidden md:table-cell">Added</Th>
								<Th className="w-10" />
							</tr>
						</thead>
						<tbody>
							{(users.data?.users ?? []).map((u) => (
								<Tr key={u.id}>
									<Td className="font-medium">
										{u.username}
										{u.id === me.data?.id && <span className="ml-1.5 text-xs text-muted">(you)</span>}
									</Td>
									<Td>
										<Select
											className="h-7 w-28"
											value={u.role}
											onChange={(e) => role.mutate({ id: u.id, role: e.target.value as Role })}
											aria-label={`Role of ${u.username}`}
										>
											{ROLES.map((r) => (
												<option key={r}>{r}</option>
											))}
										</Select>
									</Td>
									<Td>
										<EmailCell id={u.id} email={u.email} />
									</Td>
									<Td className="whitespace-normal">
										<span className="flex flex-wrap gap-1">
											{u.identities.length === 0 && <span className="text-muted">Password</span>}
											{u.identities.map((i) => (
												<span key={i.provider} className="chip">
													{i.label}
													{i.email && <span className="text-muted">{i.email}</span>}
													<button
														className="text-muted hover:text-bad"
														aria-label={`Unlink ${i.label}`}
														title="Unlink"
														onClick={() => unlink.mutate({ id: u.id, provider: i.provider })}
													>
														<Unlink className="size-3" />
													</button>
												</span>
											))}
										</span>
									</Td>
									<Td className="hidden text-muted md:table-cell">{ago(u.created)}</Td>
									<Td>
										<Button
											size="icon"
											variant="ghost"
											aria-label={`Delete ${u.username}`}
											disabled={u.id === me.data?.id}
											onClick={() => setDeleting(u)}
										>
											<Trash2 />
										</Button>
									</Td>
								</Tr>
							))}
						</tbody>
					</Table>
				</div>
				<Panel title="Add a user">
					<form
						className="grid gap-3"
						onSubmit={(e) => {
							e.preventDefault();
							const f = e.currentTarget;
							const d = new FormData(f);
							add.mutate(
								{
									username: String(d.get('username')),
									email: String(d.get('email')),
									password: String(d.get('password')),
									role: String(d.get('role')),
								},
								{ onSuccess: () => f.reset() },
							);
						}}
					>
						<Field label="User name">
							<Input name="username" required autoComplete="off" />
						</Field>
						<Field label="Email" hint="Links their Microsoft, Google or GitHub sign-in on first use.">
							<Input name="email" type="email" autoComplete="off" />
						</Field>
						<Field label={sso ? 'Password (optional with SSO)' : 'Password'}>
							<Input name="password" type="password" minLength={8} required={!sso} autoComplete="new-password" />
						</Field>
						<Field label="Role">
							<Select name="role" defaultValue="operator">
								{ROLES.map((r) => (
									<option key={r}>{r}</option>
								))}
							</Select>
						</Field>
						<Button type="submit" variant="primary">
							<Plus /> Add user
						</Button>
					</form>
				</Panel>
			</div>
			<Confirm
				open={!!deleting}
				onOpenChange={(v) => !v && setDeleting(null)}
				title={`Delete ${deleting?.username}?`}
				action="Delete"
				danger
				onConfirm={() => deleting && del.mutate(deleting.id)}
			/>
		</AdminOnly>
	);
}

// --- audit ---

export function AuditPage() {
	const [action, setAction] = React.useState('');
	const [q, setQ] = React.useState('');
	const [text, setText] = React.useState('');
	const [extra, setExtra] = React.useState<AuditRow[]>([]);
	const [next, setNext] = React.useState<number | null>(null);
	const page = useAudit({ action, q });
	React.useEffect(() => {
		setExtra([]);
		setNext(page.data?.next ?? null);
	}, [page.data]);
	React.useEffect(() => {
		const t = setTimeout(() => setQ(text), 250);
		return () => clearTimeout(t);
	}, [text]);
	async function more() {
		if (!next) return;
		const p = await api.get<AuditPageData>(
			`/audit?${new URLSearchParams({ action, q, before: String(next), limit: '100' })}`,
		);
		setExtra((x) => [...x, ...p.rows]);
		setNext(p.next);
	}
	const rows = [...(page.data?.rows ?? []), ...extra];
	return (
		<AdminOnly>
			<PageHeader title="Audit log" sub="Who did what, newest first." />
			<div className="mb-3 flex flex-wrap gap-2">
				<Input value={text} onChange={(e) => setText(e.target.value)} placeholder="Search user, device, detail…" className="w-64" aria-label="Search" />
				<Select className="w-auto" value={action} onChange={(e) => setAction(e.target.value)} aria-label="Action">
					<option value="">All actions</option>
					{(page.data?.actions ?? []).map((a) => (
						<option key={a}>{a}</option>
					))}
				</Select>
			</div>
			<div className="rounded border border-line bg-surface">
				{rows.length === 0 ? (
					<Empty>{page.isLoading ? 'Loading…' : 'Nothing matches.'}</Empty>
				) : (
					<Table>
						<thead>
							<tr>
								<Th>When</Th>
								<Th>User</Th>
								<Th>Action</Th>
								<Th>Device</Th>
								<Th>Detail</Th>
							</tr>
						</thead>
						<tbody>
							{rows.map((r) => (
								<Tr key={r.id}>
									<Td className="font-mono text-xs text-muted" title={datetime(r.ts)}>
										{datetime(r.ts)}
									</Td>
									<Td>{r.user ?? <span className="text-subtle">—</span>}</Td>
									<Td>
										<Badge tone={r.action.includes('failed') || r.action.includes('delete') ? 'bad' : r.action.startsWith('login') ? 'ok' : 'neutral'}>
											{r.action}
										</Badge>
									</Td>
									<Td>{r.device ?? <span className="text-subtle">—</span>}</Td>
									<Td className="max-w-md truncate text-muted" title={r.detail}>
										{r.detail}
									</Td>
								</Tr>
							))}
						</tbody>
					</Table>
				)}
			</div>
			{next && (
				<div className="mt-3 flex justify-center">
					<Button onClick={more}>Load more</Button>
				</div>
			)}
		</AdminOnly>
	);
}
