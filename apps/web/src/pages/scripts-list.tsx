import * as React from 'react';
import { Link, useNavigate } from '@tanstack/react-router';
import { Plus } from 'lucide-react';
import { api } from '@/api/client';
import { keys, useCan, useScripts } from '@/api/hooks';
import type { Shell } from '@/api/types';
import { Field, Input, Select } from '@/components/ui/form';
import { FormDialog } from '@/components/ui/form-dialog';
import { scriptTemplate } from '@/lib/script-templates';
import { Button } from '@/components/ui/button';
import { Chip, Empty, PageHeader } from '@/components/ui/display';
import { Table, Td, Th, Tr } from '@/components/ui/table';
import { LoadMore } from '@/components/ui/load-more';
import { SearchBox, useDebounce } from '@/components/ui/search';
import { ago } from '@/lib/utils';

export function ScriptsPage() {
	const [text, setText] = React.useState('');
	const q = useDebounce(text);
	const scripts = useScripts(q);
	const canEdit = useCan('operator');
	const list = scripts.items;
	const [adding, setAdding] = React.useState(false);
	const navigate = useNavigate();
	return (
		<>
			<PageHeader
				title="Scripts"
				sub="Reusable scripts to run on devices."
				primary={
					canEdit && (
						<Button variant="primary" onClick={() => setAdding(true)}>
							<Plus /> New script
						</Button>
					)
				}
			/>
			<SearchBox value={text} onChange={setText} placeholder="Search scripts…" />
			<div className="rounded border border-line bg-surface">
				{list.length === 0 ? (
					<Empty>{scripts.isLoading ? 'Loading…' : q ? 'No scripts match.' : 'No scripts yet.'}</Empty>
				) : (
					<Table>
						<thead>
							<tr>
								<Th>Name</Th>
								<Th>Shell</Th>
								<Th>Timeout</Th>
								<Th className="hidden md:table-cell">By</Th>
								<Th className="hidden md:table-cell">Updated</Th>
							</tr>
						</thead>
						<tbody>
							{list.map((s) => (
								<Tr key={s.id}>
									<Td>
										<Link to="/scripts/$id" params={{ id: String(s.id) }} className="font-medium hover:text-signal">
											{s.name}
										</Link>
									</Td>
									<Td>
										<Chip>{s.shell}</Chip>
									</Td>
									<Td className="font-mono text-xs">{s.timeout_s}s</Td>
									<Td className="hidden text-muted md:table-cell">{s.creator ?? '—'}</Td>
									<Td className="hidden text-muted md:table-cell">{ago(s.updated)}</Td>
								</Tr>
							))}
						</tbody>
					</Table>
				)}
				<LoadMore hasNextPage={!!scripts.hasNextPage} isFetchingNextPage={scripts.isFetchingNextPage} fetchNextPage={scripts.fetchNextPage} />
			</div>
			<FormDialog<{ id: number }>
				open={adding}
				onOpenChange={setAdding}
				title="New script"
				description="Name it and pick a shell; the editor opens next."
				submitLabel="Create and edit"
				success="Script created"
				invalidate={[keys.scripts]}
				action={(f) => {
					const shell = String(f.get('shell') ?? 'powershell') as Shell;
					return api.post<{ id: number }>('/scripts', {
						name: String(f.get('name') ?? ''),
						shell,
						timeout_s: Number(f.get('timeout_s') ?? 300),
						body: scriptTemplate[shell],
					});
				}}
				onCreated={(r) => navigate({ to: '/scripts/$id', params: { id: String(r.id) } })}
			>
				<Field label="Name">
					<Input name="name" required maxLength={200} autoFocus placeholder="e.g. Clear print queue" />
				</Field>
				<div className="grid gap-3 sm:grid-cols-2">
					<Field label="Shell">
						<Select name="shell" defaultValue="powershell">
							<option value="powershell">PowerShell</option>
							<option value="cmd">cmd</option>
							<option value="bash">bash</option>
							<option value="sh">sh</option>
						</Select>
					</Field>
					<Field label="Timeout (s)">
						<Input name="timeout_s" type="number" min={1} max={86400} defaultValue={300} required />
					</Field>
				</div>
			</FormDialog>
		</>
	);
}
