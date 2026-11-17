import * as React from 'react';
import { useQuery } from '@tanstack/react-query';
import { Bot, Laptop, Plus, ShieldAlert } from 'lucide-react';
import { api, ApiError } from '@/api/client';
import { useAction, useCan, useMe } from '@/api/hooks';
import { Button } from '@/components/ui/button';
import { Confirm } from '@/components/ui/confirm';
import { Badge, Empty, PageHeader } from '@/components/ui/display';
import { Checkbox, Field, Input, Select } from '@/components/ui/form';
import { FormDialog, useNewRow } from '@/components/ui/form-dialog';
import { Table, Td, Th, Tr } from '@/components/ui/table';
import { CopyBlock } from '@/pages/admin';
import { AuthFrame, ErrorLine } from '@/pages/auth';
import { ago, until } from '@/lib/utils';

const READ = 'deskhand:read';
const ACT = 'deskhand:act';

interface McpInfo {
	url: string;
	scopes: string[];
}

interface Connection {
	id: string;
	kind: 'oauth' | 'api';
	name: string | null;
	scopes: string[];
	created: number;
	last_used: number | null;
	expires: number | null;
	user_id: number;
	username: string;
}

interface NewToken {
	id: string;
	token: string;
	scopes: string[];
}

const mcpKeys = {
	info: ['mcp'] as const,
	connections: ['mcp-connections'] as const,
};

/** `claude mcp add` and a generic `mcpServers` entry, with a token or without. */
function snippets(url: string, token?: string) {
	const header = token ? ` --header "Authorization: Bearer ${token}"` : '';
	const json = {
		mcpServers: {
			deskhand: token ? { type: 'http', url, headers: { Authorization: `Bearer ${token}` } } : { type: 'http', url },
		},
	};
	return { claudeCode: `claude mcp add --transport http deskhand ${url}${header}`, json: JSON.stringify(json, null, 2) };
}

function Access({ scopes }: { scopes: string[] }) {
	return scopes.includes(ACT) ? <Badge tone="signal">Read and act</Badge> : <Badge>Read only</Badge>;
}

// --- the page people connect clients from ---

export function McpPage() {
	const isAdmin = useCan('admin');
	const me = useMe();
	const [everyone, setEveryone] = React.useState(false);
	const info = useQuery({ queryKey: mcpKeys.info, queryFn: () => api.get<McpInfo>('/mcp') });
	const list = useQuery({
		queryKey: [...mcpKeys.connections, everyone],
		queryFn: () => api.get<{ items: Connection[] }>(`/mcp/connections${everyone ? '?all=1' : ''}`),
	});
	const [adding, setAdding] = React.useState(false);
	const [act, setAct] = React.useState(true);
	const [revoking, setRevoking] = React.useState<Connection | null>(null);
	const fresh = useNewRow<string>();
	const revoke = useAction((id: string) => api.del(`/mcp/connections/${encodeURIComponent(id)}`), {
		invalidate: [mcpKeys.connections],
		success: 'Disconnected',
		onSuccess: () => setRevoking(null),
	});
	const canAct = useCan('operator');
	const url = info.data?.url ?? '';
	const s = snippets(url);
	const items = list.data?.items ?? [];

	return (
		<>
			<PageHeader
				title="MCP"
				sub="Connect Claude, ChatGPT or an editor to this server. A connection acts as you, with your role, and everything it does is in the audit log under your name."
				primary={
					<Button
						variant="primary"
						onClick={() => {
							setAct(canAct);
							setAdding(true);
						}}
					>
						<Plus /> New token
					</Button>
				}
			/>
			<div className="grid gap-4 lg:grid-cols-2">
				<section className="grid content-start gap-3 rounded border border-line bg-surface p-4">
					<h2 className="flex items-center gap-2 text-[15px] font-semibold">
						<Bot className="size-4 text-signal" /> Sign in from the client
					</h2>
					<p className="text-[13px] text-muted">
						Give the client this address. It registers itself, opens this server's sign-in in your browser, and asks
						you to allow it. Nothing to paste.
					</p>
					{url && <CopyBlock label="Server URL" text={url} />}
					{url && <CopyBlock label="Claude Code (then /mcp to sign in)" text={s.claudeCode} />}
					<ul className="grid gap-1 text-[13px] text-muted">
						<li>
							<b className="font-medium text-fg">Claude</b>: Settings → Connectors → Add custom connector, and paste the URL.
						</li>
						<li>
							<b className="font-medium text-fg">ChatGPT</b>: Settings → Connectors → Create, with the same URL.
						</li>
						<li>
							<b className="font-medium text-fg">VS Code, Cursor and others</b>: an HTTP MCP server with the URL.
						</li>
					</ul>
				</section>
				<section className="grid content-start gap-3 rounded border border-line bg-surface p-4">
					<h2 className="text-[15px] font-semibold">Or with an API token</h2>
					<p className="text-[13px] text-muted">
						For clients that take a header instead of signing in: scripts, CI, shared machines. Create one with{' '}
						<b className="font-medium text-fg">New token</b>; it's shown once.
					</p>
					{url && <CopyBlock label="Client config" text={snippets(url, '<token>').json} />}
					<p className="text-xs text-muted">
						{me.data?.role === 'viewer'
							? 'You are a viewer, so connections can look but not act.'
							: 'Read-only tokens can look at devices, inventory and jobs. Read and act can also run commands, scripts and power actions, as far as your role allows.'}
					</p>
				</section>
			</div>

			<section className="mt-6 grid gap-2">
				<div className="flex flex-wrap items-center justify-between gap-2">
					<h2 className="label-caps">{everyone ? 'All connections' : 'Your connections'}</h2>
					{isAdmin && (
						<label className="flex cursor-pointer items-center gap-2 text-[13px] text-muted">
							<Checkbox checked={everyone} onCheckedChange={setEveryone} aria-label="Show everyone's" /> Everyone's
						</label>
					)}
				</div>
				<div className="rounded border border-line bg-surface">
					{!items.length ? (
						<Empty>{list.isLoading ? 'Loading…' : 'Nothing connected yet.'}</Empty>
					) : (
						<Table>
							<thead>
								<tr>
									<Th>Name</Th>
									<Th>Kind</Th>
									<Th>Access</Th>
									{everyone && <Th>User</Th>}
									<Th>Connected</Th>
									<Th>Last used</Th>
									<Th>Expires</Th>
									<Th className="w-20" />
								</tr>
							</thead>
							<tbody>
								{items.map((c) => (
									<Tr key={c.id} className={fresh.rowClass(c.id)}>
										<Td className="max-w-[18rem] truncate">{c.name ?? <span className="text-muted">Unnamed client</span>}</Td>
										<Td className="text-muted">{c.kind === 'api' ? 'API token' : 'Signed in'}</Td>
										<Td>
											<Access scopes={c.scopes} />
										</Td>
										{everyone && <Td>{c.username}</Td>}
										<Td className="text-muted">{ago(c.created)}</Td>
										<Td className="text-muted">{c.last_used ? ago(c.last_used) : 'Never'}</Td>
										<Td className="text-muted">{c.kind === 'api' ? (c.expires ? until(c.expires) : 'Never') : 'Refreshes'}</Td>
										<Td>
											<Button size="sm" variant="danger" onClick={() => setRevoking(c)}>
												Revoke
											</Button>
										</Td>
									</Tr>
								))}
							</tbody>
						</Table>
					)}
				</div>
			</section>

			<Confirm
				open={!!revoking}
				onOpenChange={(v) => !v && setRevoking(null)}
				title="Revoke this connection?"
				body={
					<p className="text-[13px] text-muted">
						{revoking?.name ?? 'The client'} stops working at once. To use it again, connect it again.
					</p>
				}
				action="Revoke"
				danger
				busy={revoke.isPending}
				onConfirm={() => revoking && revoke.mutate(revoking.id)}
			/>

			<FormDialog<NewToken>
				open={adding}
				onOpenChange={setAdding}
				title="New API token"
				description="It acts as you. Name it after where it's used, so you know what to revoke."
				submitLabel="Create token"
				success="Token created"
				invalidate={[mcpKeys.connections]}
				className="md:max-w-2xl"
				action={(f) => {
					const days = String(f.get('days'));
					return api.post<NewToken>('/mcp/tokens', {
						name: f.get('name'),
						scopes: act ? [READ, ACT] : [READ],
						days: days === 'never' ? null : Number(days),
					});
				}}
				onCreated={(t) => fresh.mark(t.id)}
				successView={(t) => {
					const ts = snippets(url, t.token);
					return (
						<>
							<p className="text-[13px] text-muted">The token is shown once. Store it where the client reads it.</p>
							<CopyBlock label="Token" text={t.token} />
							<CopyBlock label="Claude Code" text={ts.claudeCode} />
							<CopyBlock label="Client config" text={ts.json} />
						</>
					);
				}}
			>
				<Field label="Name">
					<Input name="name" placeholder="e.g. CI, my laptop's Claude Desktop" maxLength={100} required autoFocus />
				</Field>
				<div className="grid gap-3 sm:grid-cols-2">
					<Field label="Expires">
						<Select name="days" defaultValue="90">
							<option value="7">In 7 days</option>
							<option value="30">In 30 days</option>
							<option value="90">In 90 days</option>
							<option value="365">In a year</option>
							<option value="never">Never</option>
						</Select>
					</Field>
					<div className="grid gap-1">
						<span className="label-caps">Access</span>
						<label className="flex h-control cursor-pointer items-center gap-2 text-[13px]">
							<Checkbox checked={act} onCheckedChange={setAct} aria-label="Allow acting" />
							Run commands, scripts and power actions
						</label>
					</div>
				</div>
			</FormDialog>
		</>
	);
}

// --- the consent page an MCP client sends the browser to ---

interface ConsentInfo {
	client_name: string | null;
	redirect_host: string;
	local: boolean;
	scopes: string[];
	username: string;
	role: string;
}

export function ConnectPage() {
	const search = window.location.search;
	const params = React.useMemo(() => Object.fromEntries(new URLSearchParams(search)), [search]);
	const info = useQuery({
		queryKey: ['mcp-consent', search],
		queryFn: () => api.get<ConsentInfo>(`/mcp/authorize${search}`),
		retry: false,
	});
	const [act, setAct] = React.useState(true);
	const [busy, setBusy] = React.useState(false);
	const [error, setError] = React.useState<string | null>(null);

	const d = info.data;
	const offered = !!d?.scopes.includes(ACT) && d.role !== 'viewer';
	const unauthorized = info.error instanceof ApiError && info.error.status === 401;
	React.useEffect(() => {
		if (!unauthorized) return;
		const here = `/connect${search}`;
		window.location.replace(`/login?next=${encodeURIComponent(here)}`);
	}, [unauthorized, search]);

	async function decide(allow: boolean) {
		setBusy(true);
		setError(null);
		try {
			const granted = allow ? [READ, ...(act && offered ? [ACT] : [])] : [];
			const r = await api.post<{ redirect: string }>('/mcp/authorize', { ...params, allow, scopes: granted });
			window.location.assign(r.redirect);
		} catch (e) {
			setError(e instanceof ApiError ? e.message : String(e));
			setBusy(false);
		}
	}

	if (!d) {
		return (
			<AuthFrame title="Connect a client">
				{info.isError && !unauthorized ? (
					<ErrorLine msg={info.error instanceof ApiError ? info.error.message : 'This link is broken.'} />
				) : (
					<p className="text-[13px] text-muted">{unauthorized ? 'Sign in first…' : 'Loading…'}</p>
				)}
			</AuthFrame>
		);
	}
	return (
		<AuthFrame title="Connect this client?">
			<ErrorLine msg={error} />
			<div className="grid gap-3 text-[13px]">
				<p className="flex items-start gap-2">
					<Laptop className="mt-0.5 size-4 shrink-0 text-muted" />
					<span>
						{d.local ? (
							<b className="font-medium">A program on this computer</b>
						) : (
							<b className="font-mono font-medium">{d.redirect_host}</b>
						)}{' '}
						wants to use Deskhand as <b className="font-medium">{d.username}</b> ({d.role}).
					</span>
				</p>
				{d.client_name && <p className="text-muted">It calls itself “{d.client_name}”. That name is the client's own claim.</p>}
				{d.local && (
					<p className="flex items-start gap-2 rounded border border-warn/40 bg-warn/10 px-3 py-2">
						<ShieldAlert className="mt-0.5 size-4 shrink-0 text-warn" />
						Any program on this computer can ask this. Allow it only if you just started connecting a client.
					</p>
				)}
				<div className="grid gap-2 rounded border border-line bg-surface p-3">
					<label className="flex items-center gap-2">
						<Checkbox checked onCheckedChange={() => {}} aria-label="Read" className="pointer-events-none opacity-60" />
						See devices, inventory, scripts and jobs
					</label>
					{offered && (
						<label className="flex cursor-pointer items-center gap-2">
							<Checkbox checked={act} onCheckedChange={setAct} aria-label="Act" />
							Run commands and scripts, power actions, refresh inventory
						</label>
					)}
				</div>
				<p className="text-xs text-muted">You can revoke it any time under MCP.</p>
				<div className="grid grid-cols-2 gap-2">
					<Button variant="secondary" size="lg" disabled={busy} onClick={() => decide(false)}>
						Deny
					</Button>
					<Button variant="primary" size="lg" disabled={busy} onClick={() => decide(true)}>
						Allow
					</Button>
				</div>
			</div>
		</AuthFrame>
	);
}
