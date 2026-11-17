import * as React from 'react';
import { Link, useNavigate, useParams } from '@tanstack/react-router';
import { useQueryClient, type InfiniteData } from '@tanstack/react-query';
import { ChevronRight, Pause, Play, Plus, RotateCw, Trash2 } from 'lucide-react';
import { api } from '@/api/client';
import { keys, useAction, useCan, useJob, useJobs, useRuns } from '@/api/hooks';
import type { JobEvent, Page, Run } from '@/api/types';
import { Input, Select } from '@/components/ui/form';
import { LoadMore } from '@/components/ui/load-more';
import { SearchBox, useDebounce } from '@/components/ui/search';
import { RunDialog } from '@/components/jobs';
import { Button } from '@/components/ui/button';
import { Confirm } from '@/components/ui/confirm';
import { Badge, Chip, Empty, KV, PageHeader, Panel, StatusBadge } from '@/components/ui/display';
import { Table, Td, Th, Tr } from '@/components/ui/table';
import { ago, cn, datetime, duration, until } from '@/lib/utils';

export function JobsPage() {
	const [text, setText] = React.useState('');
	const q = useDebounce(text);
	const jobs = useJobs(q);
	const canRun = useCan('operator');
	const [open, setOpen] = React.useState(false);
	const list = jobs.items;
	return (
		<>
			<PageHeader
				title="Jobs"
				sub="Scripts and power actions, run now or on a schedule."
				primary={
					canRun && (
						<Button variant="primary" onClick={() => setOpen(true)}>
							<Plus /> New job
						</Button>
					)
				}
			/>
			<SearchBox value={text} onChange={setText} placeholder="Search jobs…" />
			<div className="rounded border border-line bg-surface">
				{list.length === 0 ? (
					<Empty>{jobs.isLoading ? 'Loading…' : q ? 'No jobs match.' : 'No jobs yet.'}</Empty>
				) : (
					<Table>
						<thead>
							<tr>
								<Th>Job</Th>
								<Th>Task</Th>
								<Th className="hidden md:table-cell">Target</Th>
								<Th>Results</Th>
								<Th className="hidden lg:table-cell">Schedule</Th>
								<Th className="hidden lg:table-cell">By</Th>
							</tr>
						</thead>
						<tbody>
							{list.map((j) => {
								const c = j.counts!;
								return (
									<Tr key={j.id}>
										<Td className="max-w-72">
											<Link to="/jobs/$id" params={{ id: String(j.id) }} className="block truncate font-medium hover:text-signal">
												{j.name}
											</Link>
										</Td>
										<Td>
											<Chip>{j.task}</Chip>
										</Td>
										<Td className="hidden max-w-56 truncate text-muted md:table-cell">{j.target}</Td>
										<Td className="font-mono text-xs tabular">
											<span className="text-ok">{c.ok}</span>
											<span className="text-subtle"> / </span>
											<span className={c.failed + c.timeout ? 'text-bad' : 'text-subtle'}>{c.failed + c.timeout}</span>
											<span className="text-subtle"> / </span>
											<span className={c.running + c.queued ? 'text-run' : 'text-subtle'}>{c.running + c.queued}</span>
										</Td>
										<Td className="hidden lg:table-cell">
											{j.schedule ? (
												<span className="flex items-center gap-2">
													<Chip>{j.schedule}</Chip>
													{!j.enabled ? <Badge>paused</Badge> : <span className="text-xs text-muted">{until(j.next_fire)}</span>}
												</span>
											) : (
												<span className="text-muted">once · {ago(j.last_fired ?? j.created)}</span>
											)}
										</Td>
										<Td className="hidden text-muted lg:table-cell">{j.creator ?? '—'}</Td>
									</Tr>
								);
							})}
						</tbody>
					</Table>
				)}
				<LoadMore hasNextPage={!!jobs.hasNextPage} isFetchingNextPage={jobs.isFetchingNextPage} fetchNextPage={jobs.fetchNextPage} />
			</div>
			<p className="mt-2 text-xs text-muted">Results: ok / failed / pending.</p>
			<RunDialog open={open} onOpenChange={setOpen} />
		</>
	);
}

/** Applies a job event to one loaded run. */
function applyEvent(r: Run, e: JobEvent): Run {
	if (e.kind === 'new_runs' || r.id !== e.run_id) return r;
	if (e.kind === 'status') {
		return { ...r, status: e.status, exit_code: e.exit_code, started: e.started, finished: e.finished, stdout: e.stdout, stderr: e.stderr };
	}
	const key = e.stream;
	const before = r[key];
	// `end` is the stream's length after this chunk: skip what we already have.
	if (before.length >= e.end) return r;
	return { ...r, [key]: before.slice(0, Math.max(0, e.end - e.data.length)) + e.data };
}

/** Keeps the job's loaded runs live; new runs or status changes refresh the counts. */
function useJobEvents(id: number) {
	const qc = useQueryClient();
	React.useEffect(() => {
		const es = new EventSource(`/api/v1/jobs/${id}/events`);
		let t: number | undefined;
		const refreshCounts = () => {
			window.clearTimeout(t);
			t = window.setTimeout(() => qc.invalidateQueries({ queryKey: keys.job(id) }), 300);
		};
		es.onmessage = (m) => {
			const e = JSON.parse(m.data) as JobEvent;
			if (e.kind === 'new_runs') {
				qc.invalidateQueries({ queryKey: keys.runs(id) });
				refreshCounts();
				return;
			}
			qc.setQueriesData<InfiniteData<Page<Run>>>({ queryKey: keys.runs(id) }, (data) =>
				data ? { ...data, pages: data.pages.map((p) => ({ ...p, items: p.items.map((r) => applyEvent(r, e)) })) } : data,
			);
			if (e.kind === 'status') refreshCounts();
		};
		return () => {
			es.close();
			window.clearTimeout(t);
		};
	}, [id, qc]);
}

function Output({ r }: { r: Run }) {
	const ref = React.useRef<HTMLPreElement>(null);
	React.useEffect(() => {
		const el = ref.current;
		if (el) el.scrollTop = el.scrollHeight;
	}, [r.stdout, r.stderr]);
	if (!r.stdout && !r.stderr) {
		return <div className="px-3 py-2 text-xs text-muted">{r.status === 'queued' ? 'Waiting for the device…' : 'No output.'}</div>;
	}
	return (
		<pre
			ref={ref}
			className="max-h-80 overflow-auto bg-term px-3 py-2 font-mono text-xs leading-relaxed text-term-fg"
		>
			{r.stdout}
			{r.stderr && <span className="text-term-err">{r.stderr}</span>}
		</pre>
	);
}

export function JobPage() {
	const { id: raw } = useParams({ from: '/app/jobs/$id' });
	const id = Number(raw);
	const q = useJob(id);
	const [status, setStatus] = React.useState('');
	const [text, setText] = React.useState('');
	const runQ = useDebounce(text);
	const runsQ = useRuns(id, { status, q: runQ });
	const runs = runsQ.items;
	const canRun = useCan('operator');
	const navigate = useNavigate();
	const [open, setOpen] = React.useState<Set<number>>(new Set());
	const [deleting, setDeleting] = React.useState(false);
	useJobEvents(id);

	const op = useAction((o: string) => api.post(`/jobs/${id}/${o}`), {
		invalidate: [keys.job(id), keys.jobs, keys.runs(id)],
		success: (_, o) => ({ run: 'Running again', enable: 'Resumed', disable: 'Paused' })[o] ?? 'Done',
	});
	const del = useAction(() => api.del(`/jobs/${id}`), {
		invalidate: [keys.jobs],
		success: 'Job deleted',
		onSuccess: () => navigate({ to: '/jobs' }),
	});

	// Open the output of a single-device job, and of anything that fails.
	const only = runs.length === 1 && !runsQ.hasNextPage ? runs[0].id : null;
	React.useEffect(() => {
		if (only != null) setOpen((o) => new Set(o).add(only));
	}, [only]);

	if (q.isLoading) return <Empty>Loading…</Empty>;
	if (!q.data) return <Empty>That job doesn't exist.</Empty>;
	const { job: j, counts } = q.data;
	const totalRuns = Object.values(counts).reduce((a, b) => a + b, 0);

	return (
		<>
			<PageHeader
				title={j.name}
				sub={
					<Link to="/jobs" className="hover:text-fg">
						Jobs
					</Link>
				}
				actions={
					canRun && (
						<>
							<Button onClick={() => op.mutate('run')}>
								<RotateCw /> Run again
							</Button>
							{j.schedule &&
								(j.enabled ? (
									<Button onClick={() => op.mutate('disable')}>
										<Pause /> Pause
									</Button>
								) : (
									<Button onClick={() => op.mutate('enable')}>
										<Play /> Resume
									</Button>
								))}
							<Button variant="danger" onClick={() => setDeleting(true)}>
								<Trash2 /> Delete
							</Button>
						</>
					)
				}
			/>
			<div className="grid gap-3 lg:grid-cols-[1fr_320px]">
				<Panel
					title={`Runs (${totalRuns})`}
					actions={
						<span className="flex gap-1">
							{Object.entries(counts).map(([s, n]) => (
								<Badge key={s} tone={s === 'ok' ? 'ok' : s === 'failed' || s === 'timeout' ? 'bad' : s === 'running' ? 'run' : s === 'queued' ? 'warn' : 'off'}>
									{n} {s}
								</Badge>
							))}
						</span>
					}
					bodyClassName="p-0"
				>
					<div className="flex flex-wrap gap-2 border-b border-line p-2">
						<Input
							type="search"
							value={text}
							onChange={(e) => setText(e.target.value)}
							placeholder="Search devices…"
							aria-label="Search runs by device"
							className="max-w-56"
						/>
						<Select className="w-auto" value={status} onChange={(e) => setStatus(e.target.value)} aria-label="Run status">
							<option value="">Any status</option>
							{['queued', 'running', 'ok', 'failed', 'timeout', 'offline'].map((s) => (
								<option key={s} value={s}>
									{s} {counts[s] ? `(${counts[s]})` : ''}
								</option>
							))}
						</Select>
					</div>
					{runs.length === 0 ? (
						<Empty>
							{runsQ.isLoading ? 'Loading…' : status || runQ ? 'No runs match.' : `No runs yet${j.schedule ? `; next ${until(j.next_fire)}` : ''}.`}
						</Empty>
					) : (
						<Table>
							<thead>
								<tr>
									<Th className="w-6" />
									<Th>Device</Th>
									<Th>Status</Th>
									<Th>Exit</Th>
									<Th>Duration</Th>
									<Th className="hidden md:table-cell">Queued</Th>
								</tr>
							</thead>
							<tbody>
								{runs.map((r) => {
									const isOpen = open.has(r.id);
									return (
										<React.Fragment key={r.id}>
											<Tr
												className="cursor-pointer"
												onClick={() =>
													setOpen((o) => {
														const n = new Set(o);
														if (n.has(r.id)) n.delete(r.id);
														else n.add(r.id);
														return n;
													})
												}
											>
												<Td className="pr-0">
													<ChevronRight className={cn('size-4 text-muted transition-transform', isOpen && 'rotate-90')} />
												</Td>
												<Td>
													<Link
														to="/devices/$id"
														params={{ id: r.device_id }}
														search={{ tab: undefined }}
														className="font-medium hover:text-signal"
														onClick={(e) => e.stopPropagation()}
													>
														{r.device_name}
													</Link>
													{r.via && <span className="ml-2 text-xs text-muted">via {r.via}</span>}
												</Td>
												<Td>
													<StatusBadge status={r.status} />
												</Td>
												<Td className="font-mono text-xs">{r.exit_code ?? '—'}</Td>
												<Td className="font-mono text-xs">{duration(r.started, r.finished)}</Td>
												<Td className="hidden text-muted md:table-cell">{ago(r.queued)}</Td>
											</Tr>
											{isOpen && (
												<tr>
													<td colSpan={6} className="border-b border-line p-0">
														<Output r={r} />
													</td>
												</tr>
											)}
										</React.Fragment>
									);
								})}
							</tbody>
						</Table>
					)}
					<LoadMore hasNextPage={!!runsQ.hasNextPage} isFetchingNextPage={runsQ.isFetchingNextPage} fetchNextPage={runsQ.fetchNextPage} />
				</Panel>
				<Panel title="Details">
					<KV
						rows={[
							['Task', <Chip key="t">{j.task}</Chip>],
							[
								'Script',
								j.script_id ? (
									<Link key="s" to="/scripts/$id" params={{ id: String(j.script_id) }} className="text-signal hover:underline">
										Open script
									</Link>
								) : (
									'—'
								),
							],
							['Target', j.target],
							['Schedule', j.schedule ? <Chip key="c">{j.schedule}</Chip> : 'Once'],
							['Next', j.schedule ? (j.enabled ? datetime(j.next_fire) : 'Paused') : '—'],
							['Timeout', `${j.timeout_s}s`],
							['Offline devices', `wait up to ${Math.round(j.expires_after / 3600)}h`],
							['Created', `${datetime(j.created)} by ${j.creator ?? '—'}`],
						]}
					/>
				</Panel>
			</div>
			<Confirm
				open={deleting}
				onOpenChange={setDeleting}
				title="Delete this job?"
				body="Its runs and their output go with it."
				action="Delete"
				danger
				busy={del.isPending}
				onConfirm={() => del.mutate(undefined)}
			/>
		</>
	);
}
