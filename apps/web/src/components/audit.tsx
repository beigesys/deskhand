import { Link } from '@tanstack/react-router';
import type { AuditRow } from '@/api/types';
import { Badge, Empty, type Tone } from '@/components/ui/display';
import { Table, Td, Th, Tr } from '@/components/ui/table';
import { datetime } from '@/lib/utils';

function actionTone(a: string): Tone {
	if (a.includes('failed') || a.includes('denied') || a.endsWith('.delete') || a.startsWith('mfa.reset') || a.endsWith('.disable'))
		return 'bad';
	if (a.startsWith('power.')) return 'warn';
	if (a.startsWith('login') || a.startsWith('sso.login') || a === 'mfa.enable') return 'ok';
	if (a.startsWith('desktop.') || a.startsWith('shell.')) return 'signal';
	return 'neutral';
}

const kindLabel: Record<string, string> = {
	user: 'User',
	group: 'Group',
	enroll_token: 'Token',
	job: 'Job',
	script: 'Script',
	device: 'Device',
};

/** What an entry was done to: a link when the target still exists and has a page. */
function Target({ r }: { r: AuditRow }) {
	if (!r.target_kind) return <span className="text-subtle">—</span>;
	const name = r.target_name ?? (r.target_kind === 'device' ? r.device : null) ?? `#${r.target_id}`;
	const kind = <span className="mr-1.5 text-[11px] uppercase tracking-wide text-subtle">{kindLabel[r.target_kind] ?? r.target_kind}</span>;
	const id = r.target_id ?? '';
	const gone = !r.target_name && !(r.target_kind === 'device' && r.device_id && r.device);
	const link =
		gone || !id ? null : r.target_kind === 'device' ? (
			<Link to="/devices/$id" params={{ id }} className="hover:underline">
				{name}
			</Link>
		) : r.target_kind === 'job' ? (
			<Link to="/jobs/$id" params={{ id }} className="hover:underline">
				{name}
			</Link>
		) : r.target_kind === 'script' ? (
			<Link to="/scripts/$id" params={{ id }} className="hover:underline">
				{name}
			</Link>
		) : null;
	return (
		<span className="whitespace-nowrap">
			{kind}
			{link ?? <span className={gone ? 'text-muted line-through decoration-subtle' : undefined}>{name}</span>}
		</span>
	);
}

/** Audit entries, newest first. `showTarget` off for a single device's activity. */
export function AuditTable({ rows, loading, showTarget = true }: { rows: AuditRow[]; loading: boolean; showTarget?: boolean }) {
	if (rows.length === 0) return <Empty>{loading ? 'Loading…' : 'Nothing matches.'}</Empty>;
	return (
		<Table>
			<thead>
				<tr>
					<Th>When</Th>
					<Th>User</Th>
					<Th>Action</Th>
					{showTarget && <Th>Target</Th>}
					<Th>Detail</Th>
					<Th className="hidden lg:table-cell">IP</Th>
				</tr>
			</thead>
			<tbody>
				{rows.map((r) => (
					<Tr key={r.id}>
						<Td className="font-mono text-xs text-muted">{datetime(r.ts)}</Td>
						<Td>{r.user ?? <span className="text-subtle">—</span>}</Td>
						<Td>
							<Badge tone={actionTone(r.action)}>{r.action}</Badge>
						</Td>
						{showTarget && (
							<Td>
								<Target r={r} />
							</Td>
						)}
						<Td className="max-w-md truncate text-muted" title={r.detail}>
							{r.detail}
						</Td>
						<Td className="hidden font-mono text-xs text-muted lg:table-cell">{r.ip ?? ''}</Td>
					</Tr>
				))}
			</tbody>
		</Table>
	);
}
