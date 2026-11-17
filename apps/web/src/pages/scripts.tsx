import * as React from 'react';
import { Link, useNavigate, useParams } from '@tanstack/react-router';
import CodeMirror from '@uiw/react-codemirror';
import { StreamLanguage } from '@codemirror/language';
import { EditorView } from '@codemirror/view';
import { shell as shellMode } from '@codemirror/legacy-modes/mode/shell';
import { powerShell } from '@codemirror/legacy-modes/mode/powershell';
import { Play, Save, Trash2 } from 'lucide-react';
import { api } from '@/api/client';
import { keys, useAction, useCan, useScript } from '@/api/hooks';
import type { Shell } from '@/api/types';
import { RunDialog } from '@/components/jobs';
import { Button } from '@/components/ui/button';
import { Confirm } from '@/components/ui/confirm';
import { Empty, PageHeader } from '@/components/ui/display';
import { Field, Input, Select } from '@/components/ui/form';
import { scriptTemplate } from '@/lib/script-templates';

const editorTheme = EditorView.theme({
	'&': { backgroundColor: 'var(--dh-surface)', color: 'var(--dh-fg)', fontSize: '13px', height: '100%' },
	'.cm-content': { fontFamily: 'var(--dh-font-mono)', caretColor: 'var(--dh-signal)' },
	'.cm-gutters': {
		backgroundColor: 'var(--dh-surface-2)',
		color: 'var(--dh-subtle)',
		borderRight: '1px solid var(--dh-line)',
		fontFamily: 'var(--dh-font-mono)',
	},
	'.cm-activeLine': { backgroundColor: 'color-mix(in oklab, var(--dh-signal) 6%, transparent)' },
	'.cm-activeLineGutter': { backgroundColor: 'transparent', color: 'var(--dh-fg)' },
	'&.cm-focused': { outline: 'none' },
	'.cm-selectionBackground, &.cm-focused .cm-selectionBackground': {
		backgroundColor: 'color-mix(in oklab, var(--dh-signal) 22%, transparent) !important',
	},
	'.cm-cursor': { borderLeftColor: 'var(--dh-signal)' },
	'.cm-scroller': { minHeight: '360px' },
});

const placeholder = scriptTemplate;

export function ScriptPage() {
	const { id: raw } = useParams({ from: '/app/scripts/$id' });
	const isNew = raw === 'new';
	const id = isNew ? 0 : Number(raw);
	const q = useScript(id);
	const canEdit = useCan('operator');
	const navigate = useNavigate();
	const [name, setName] = React.useState('');
	const [shell, setShell] = React.useState<Shell>('powershell');
	const [timeout, setTimeoutS] = React.useState(300);
	const [body, setBody] = React.useState(placeholder.powershell);
	const [dirty, setDirty] = React.useState(false);
	const [run, setRun] = React.useState(false);
	const [deleting, setDeleting] = React.useState(false);

	React.useEffect(() => {
		if (q.data) {
			setName(q.data.name);
			setShell(q.data.shell);
			setTimeoutS(q.data.timeout_s);
			setBody(q.data.body ?? '');
			setDirty(false);
		}
	}, [q.data]);

	const save = useAction(
		() => {
			const b = { name, shell, body, timeout_s: timeout };
			return isNew ? api.post<{ id?: number }>('/scripts', b) : api.put<{ id?: number }>(`/scripts/${id}`, b);
		},
		{
			invalidate: [keys.scripts, keys.script(id)],
			success: 'Saved',
			onSuccess: (r) => {
				setDirty(false);
				if (isNew && r.id) navigate({ to: '/scripts/$id', params: { id: String(r.id) } });
			},
		},
	);
	const del = useAction(() => api.del(`/scripts/${id}`), {
		invalidate: [keys.scripts],
		success: 'Script deleted',
		onSuccess: () => navigate({ to: '/scripts' }),
	});

	const language = React.useMemo(
		() => StreamLanguage.define(shell === 'powershell' ? powerShell : shellMode),
		[shell],
	);

	if (!isNew && q.isLoading) return <Empty>Loading…</Empty>;
	if (!isNew && !q.data) return <Empty>That script doesn't exist.</Empty>;

	return (
		<form
			onSubmit={(e) => {
				e.preventDefault();
				save.mutate(undefined);
			}}
			onKeyDown={(e) => {
				if ((e.metaKey || e.ctrlKey) && e.key === 's') {
					e.preventDefault();
					if (canEdit) save.mutate(undefined);
				}
			}}
		>
			<PageHeader
				title={isNew ? 'New script' : name || 'Script'}
				sub={
					<Link to="/scripts" className="hover:text-fg">
						Scripts
					</Link>
				}
				actions={
					canEdit && (
						<>
							{!isNew && (
								<>
									<Button variant="danger" onClick={() => setDeleting(true)}>
										<Trash2 /> Delete
									</Button>
									<Button onClick={() => setRun(true)} disabled={dirty}>
										<Play /> Run…
									</Button>
								</>
							)}
							<Button type="submit" variant="primary" disabled={save.isPending || (!dirty && !isNew)}>
								<Save /> Save
							</Button>
						</>
					)
				}
			/>
			<div className="mb-3 grid gap-3 sm:grid-cols-[1fr_160px_140px]">
				<Field label="Name">
					<Input
						value={name}
						onChange={(e) => {
							setName(e.target.value);
							setDirty(true);
						}}
						required
						disabled={!canEdit}
					/>
				</Field>
				<Field label="Shell">
					<Select
						value={shell}
						onChange={(e) => {
							const s = e.target.value as Shell;
							if (isNew && !dirty) setBody(placeholder[s]);
							setShell(s);
							setDirty(true);
						}}
						disabled={!canEdit}
					>
						<option value="powershell">PowerShell</option>
						<option value="cmd">cmd</option>
						<option value="bash">bash</option>
						<option value="sh">sh</option>
					</Select>
				</Field>
				<Field label="Timeout (s)">
					<Input
						type="number"
						min={1}
						max={86400}
						value={timeout}
						onChange={(e) => {
							setTimeoutS(Number(e.target.value));
							setDirty(true);
						}}
						disabled={!canEdit}
					/>
				</Field>
			</div>
			<div className="overflow-hidden rounded border border-line-strong">
				<CodeMirror
					value={body}
					onChange={(v) => {
						setBody(v);
						setDirty(true);
					}}
					extensions={[language, editorTheme]}
					theme="none"
					editable={canEdit}
					basicSetup={{ foldGutter: false, highlightActiveLine: true, tabSize: 4 }}
					aria-label="Script body"
				/>
			</div>
			<p className="mt-2 text-xs text-muted">
				{dirty ? 'Unsaved changes. ' : ''}Ctrl+S saves. Runs as the agent's user.
			</p>
			{q.data && <RunDialog open={run} onOpenChange={setRun} initial={{ source: `script:${id}` }} />}
			<Confirm
				open={deleting}
				onOpenChange={setDeleting}
				title={`Delete ${name}?`}
				body="Jobs that already ran it keep their copy."
				action="Delete"
				danger
				busy={del.isPending}
				onConfirm={() => del.mutate(undefined)}
			/>
		</form>
	);
}
