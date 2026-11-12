import { Link } from '@tanstack/react-router';
import { Plus } from 'lucide-react';
import { useCan, useScripts } from '@/api/hooks';
import { Button } from '@/components/ui/button';
import { Chip, Empty, PageHeader } from '@/components/ui/display';
import { Table, Td, Th, Tr } from '@/components/ui/table';
import { ago } from '@/lib/utils';

export function ScriptsPage() {
	const scripts = useScripts();
	const canEdit = useCan('operator');
	const list = scripts.data ?? [];
	return (
		<>
			<PageHeader
				title="Scripts"
				sub="Reusable scripts to run on devices."
				actions={
					canEdit && (
						<Button variant="primary" asChild>
							<Link to="/scripts/$id" params={{ id: 'new' }}>
								<Plus /> New script
							</Link>
						</Button>
					)
				}
			/>
			<div className="rounded border border-line bg-surface">
				{list.length === 0 ? (
					<Empty>{scripts.isLoading ? 'Loading…' : 'No scripts yet.'}</Empty>
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
			</div>
		</>
	);
}
