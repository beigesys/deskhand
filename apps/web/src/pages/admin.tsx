import * as React from 'react';
import { Link } from '@tanstack/react-router';
import { AlertTriangle, Check, Copy, Plus, Trash2, Unlink } from 'lucide-react';
import { api } from '@/api/client';
import { keys, useAction, useAgentDownloads, useAudit, useCan, useGroups, useMe, useSsoEnabled, useTokens, useUsers } from '@/api/hooks';
import type { AgentBinary, Device, Group, NewEnrollToken, Role, User } from '@/api/types';
import { AsyncCombobox, deviceOption, groupOption, NO_GROUP, userOption, type Option } from '@/components/ui/async-combobox';
import { LoadMore } from '@/components/ui/load-more';
import { SearchBox, useDebounce } from '@/components/ui/search';
import { Button } from '@/components/ui/button';
import { Confirm } from '@/components/ui/confirm';
import { Badge, Empty, PageHeader } from '@/components/ui/display';
import { Field, Input, Select } from '@/components/ui/form';
import { FormDialog, useNewRow } from '@/components/ui/form-dialog';
import { Table, Td, Th, Tr } from '@/components/ui/table';
import { ago, datetime, until } from '@/lib/utils';

function AdminOnly({ children }: { children: React.ReactNode }) {
	const ok = useCan('admin');
	return ok ? <>{children}</> : <Empty>Only admins can see this page.</Empty>;
}

// --- groups ---

export function GroupsPage() {
	const [text, setText] = React.useState('');
	const q = useDebounce(text);
	const groups = useGroups(q);
	const [adding, setAdding] = React.useState(false);
	const fresh = useNewRow<number>();
	const [deleting, setDeleting] = React.useState<{ id: number; name: string } | null>(null);
	const del = useAction((id: number) => api.del(`/groups/${id}`), {
		invalidate: [keys.groups, keys.devices],
		success: 'Group deleted',
		onSuccess: () => setDeleting(null),
	});
	return (
		<AdminOnly>
			<PageHeader
				title="Groups"
				sub="Group devices by site, team or role. Jobs and enrollment tokens can target a group."
				primary={
					<Button variant="primary" onClick={() => setAdding(true)}>
						<Plus /> Add group
					</Button>
				}
			/>
			<SearchBox value={text} onChange={setText} placeholder="Search groups…" />
			<div className="rounded border border-line bg-surface">
				{!groups.items.length ? (
					<Empty>{groups.isLoading ? 'Loading…' : q ? 'No groups match.' : 'No groups yet. Add one to sort devices by site or team.'}</Empty>
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
							{groups.items.map((g) => (
								<Tr key={g.id} className={fresh.rowClass(g.id)}>
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
				<LoadMore hasNextPage={!!groups.hasNextPage} isFetchingNextPage={groups.isFetchingNextPage} fetchNextPage={groups.fetchNextPage} />
			</div>
			<FormDialog
				open={adding}
				onOpenChange={setAdding}
				title="Add group"
				submitLabel="Add group"
				success="Group added"
				invalidate={[keys.groups]}
				action={(f) => api.post<{ id: number }>('/groups', { name: String(f.get('name') ?? '') })}
				onCreated={(r) => fresh.mark(r.id)}
			>
				<Field label="Name">
					<Input name="name" required maxLength={100} autoFocus placeholder="e.g. Clinic · Main" />
				</Field>
			</FormDialog>
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
			<pre className="overflow-x-auto whitespace-pre-wrap break-all rounded border border-line bg-term p-3 font-mono text-xs text-term-fg">
				{text}
			</pre>
		</div>
	);
}

const STATUS_TEXT: Record<AgentBinary['status'], string> = {
	cached: 'ready',
	fetched: 'ready (from release)',
	fetching: 'downloading…',
	unavailable: 'unavailable',
};

/** Which agent builds this server can hand to the install commands. */
function AgentBuilds({ binaries }: { binaries: AgentBinary[] }) {
	return (
		<ul className="grid gap-1 text-[13px]">
			{binaries.map((b) => (
				<li key={b.file} className="flex flex-wrap items-center gap-x-2 gap-y-0.5">
					<span
						className={
							b.present ? 'size-2 rounded-[1px] bg-ok' : b.status === 'fetching' ? 'size-2 rounded-[1px] bg-run' : 'size-2 rounded-[1px] bg-warn'
						}
						aria-hidden
					/>
					<span className="min-w-36">{b.platform}</span>
					<span className={b.present ? 'text-muted' : 'text-warn'}>{STATUS_TEXT[b.status]}</span>
					{b.detail && !b.present && <span className="w-full pl-4 text-xs text-muted break-words">{b.detail}</span>}
				</li>
			))}
		</ul>
	);
}

export function EnrollPage() {
	const tokens = useTokens();
	const downloads = useAgentDownloads();
	const [group, setGroup] = React.useState<string | null>(null);
	const [adding, setAdding] = React.useState(false);
	const fresh = useNewRow<number>();
	const revoke = useAction((id: number) => api.del(`/enroll-tokens/${id}`), { invalidate: [keys.tokens], success: 'Revoked' });
	const missing = downloads.data?.binaries.filter((b) => !b.present) ?? [];
	return (
		<AdminOnly>
			<PageHeader
				title="Enrollment"
				sub="Tokens let new machines join. Each machine trades one for its own secret."
				primary={
					<Button
						variant="primary"
						onClick={() => {
							setGroup(null);
							setAdding(true);
						}}
					>
						<Plus /> New token
					</Button>
				}
			/>
			{downloads.data && missing.length > 0 && (
				<div className="mb-3 flex items-start gap-2 rounded border border-warn/40 bg-warn/10 px-3 py-2 text-[13px]">
					<AlertTriangle className="mt-0.5 size-4 shrink-0 text-warn" />
					<div className="grid gap-1">
						<span>
							Install commands for {missing.map((b) => b.platform).join(', ')} won't work until this server has those agent builds. Put them in{' '}
							<span className="font-mono text-xs">{downloads.data.dir}</span> or let the server fetch them from the v{downloads.data.server_version} release.
						</span>
					</div>
				</div>
			)}
			<div className="rounded border border-line bg-surface">
				{!tokens.items.length ? (
					<Empty>{tokens.isLoading ? 'Loading…' : 'No active tokens. Create one to enroll a machine.'}</Empty>
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
							{tokens.items.map((t) => (
								<Tr key={t.id} className={fresh.rowClass(t.id)}>
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
			</div>
			{downloads.data && (
				<section className="mt-4 grid gap-2">
					<h2 className="label-caps">Agent builds on this server</h2>
					<div className="rounded border border-line bg-surface p-3">
						<AgentBuilds binaries={downloads.data.binaries} />
					</div>
				</section>
			)}
			<FormDialog<NewEnrollToken>
				open={adding}
				onOpenChange={setAdding}
				title="New enrollment token"
				description="Machines that use it join the group you pick."
				submitLabel="Create token"
				success="Token created"
				invalidate={[keys.tokens, keys.agentDownloads]}
				className="md:max-w-2xl"
				action={(f) =>
					api.post<NewEnrollToken>('/enroll-tokens', {
						group_id: group && group !== 'none' ? Number(group) : null,
						hours: Number(f.get('hours')),
						max_uses: Number(f.get('max_uses')),
					})
				}
				onCreated={(t) => fresh.mark(t.id)}
				successView={(t) => (
					<>
						<p className="text-[13px] text-muted">The token is shown once. Run one of these on the machine to install and enroll it.</p>
						<CopyBlock label="Linux / macOS (as root)" text={t.linux} />
						<CopyBlock label="Windows (PowerShell as Administrator)" text={t.windows} />
						<CopyBlock label="Token" text={t.token} />
						{t.binaries.some((b) => !b.present) && (
							<div className="rounded border border-warn/40 bg-warn/10 px-3 py-2">
								<AgentBuilds binaries={t.binaries} />
							</div>
						)}
					</>
				)}
			>
				<Field label="Group">
					<AsyncCombobox<Group>
						endpoint="/groups"
						toOption={groupOption}
						extra={[NO_GROUP]}
						value={group ?? 'none'}
						onChange={setGroup}
						searchPlaceholder="Search groups…"
						aria-label="Group"
					/>
				</Field>
				<div className="grid gap-3 sm:grid-cols-2">
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
				</div>
			</FormDialog>
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
	const [text, setText] = React.useState('');
	const q = useDebounce(text);
	const users = useUsers(q);
	const ssoQ = useSsoEnabled();
	const me = useMe();
	const [deleting, setDeleting] = React.useState<{ id: number; username: string } | null>(null);
	const [adding, setAdding] = React.useState(false);
	const fresh = useNewRow<number>();
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
	const sso = ssoQ.data ?? false;
	return (
		<AdminOnly>
			<PageHeader
				title="Users"
				sub="Viewers see devices. Operators can also open desktops and shells and run jobs. Admins manage everything."
				primary={
					<Button variant="primary" onClick={() => setAdding(true)}>
						<Plus /> Add user
					</Button>
				}
			/>
			<SearchBox value={text} onChange={setText} placeholder="Search users or email…" />
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
							{users.items.map((u) => (
								<Tr key={u.id} className={fresh.rowClass(u.id)}>
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
					<LoadMore hasNextPage={!!users.hasNextPage} isFetchingNextPage={users.isFetchingNextPage} fetchNextPage={users.fetchNextPage} />
			</div>
			<FormDialog
				open={adding}
				onOpenChange={setAdding}
				title="Add user"
				submitLabel="Add user"
				success="User added"
				invalidate={[keys.users]}
				action={(f) =>
					api.post<{ id: number }>('/users', {
						username: String(f.get('username') ?? ''),
						email: String(f.get('email') ?? '') || null,
						password: String(f.get('password') ?? '') || null,
						role: String(f.get('role') ?? 'operator'),
					})
				}
				onCreated={(r) => fresh.mark(r.id)}
			>
				<Field label="User name">
					<Input name="username" required autoComplete="off" autoFocus />
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
			</FormDialog>
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

const actionOption = (a: { id: string; name: string }): Option => ({ id: a.id, label: a.name });

export function AuditPage() {
	const [action, setAction] = React.useState<string | null>(null);
	const [user, setUser] = React.useState<string | null>(null);
	const [device, setDevice] = React.useState<string | null>(null);
	const [text, setText] = React.useState('');
	const q = useDebounce(text, 250);
	const audit = useAudit({ action: action ?? undefined, user: user ?? undefined, device: device ?? undefined, q });
	const rows = audit.items;
	const filtered = !!(action || user || device || q);
	return (
		<AdminOnly>
			<PageHeader title="Audit log" sub="Who did what, newest first." />
			<div className="mb-3 grid gap-2 sm:grid-cols-2 lg:grid-cols-[minmax(200px,1fr)_200px_200px_200px_auto]">
				<Input value={text} onChange={(e) => setText(e.target.value)} type="search" placeholder="Search user, device, detail…" aria-label="Search" />
				<AsyncCombobox<User>
					endpoint="/users"
					kind="audit-user"
					toOption={userOption}
					value={user}
					onChange={setUser}
					placeholder="Any user"
					searchPlaceholder="Search users…"
					aria-label="User"
				/>
				<AsyncCombobox<Device>
					endpoint="/devices"
					kind="audit-device"
					toOption={deviceOption}
					value={device}
					onChange={setDevice}
					placeholder="Any device"
					searchPlaceholder="Search devices…"
					aria-label="Device"
				/>
				<AsyncCombobox<{ id: string; name: string }>
					endpoint="/audit/actions"
					toOption={actionOption}
					value={action}
					onChange={setAction}
					placeholder="Any action"
					searchPlaceholder="Search actions…"
					aria-label="Action"
				/>
				{filtered && (
					<Button
						variant="ghost"
						onClick={() => {
							setAction(null);
							setUser(null);
							setDevice(null);
							setText('');
						}}
					>
						Clear
					</Button>
				)}
			</div>
			<div className="rounded border border-line bg-surface">
				{rows.length === 0 ? (
					<Empty>{audit.isLoading ? 'Loading…' : 'Nothing matches.'}</Empty>
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
									<Td className="font-mono text-xs text-muted">{datetime(r.ts)}</Td>
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
				<LoadMore hasNextPage={!!audit.hasNextPage} isFetchingNextPage={audit.isFetchingNextPage} fetchNextPage={audit.fetchNextPage} />
			</div>
		</AdminOnly>
	);
}
