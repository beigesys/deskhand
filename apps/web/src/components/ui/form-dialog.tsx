import * as React from 'react';
import { useQueryClient, type QueryKey } from '@tanstack/react-query';
import { AlertCircle } from 'lucide-react';
import { toast } from 'sonner';
import { ApiError } from '@/api/client';
import { keys } from '@/api/hooks';
import { Button } from '@/components/ui/button';
import { Dialog, DialogContent, DialogFooter } from '@/components/ui/overlay';

/**
 * Every "add / create" form: opened from the page header's primary button, submitted in a
 * dialog (a bottom sheet on phones). Errors stay in the dialog; success closes it, toasts,
 * refreshes `invalidate` and hands the result to `onCreated` (to highlight the new row),
 * or, with `successView`, keeps the dialog open to show the result.
 */
export function FormDialog<R>({
	open,
	onOpenChange,
	title,
	description,
	submitLabel,
	action,
	invalidate = [],
	success,
	onCreated,
	successView,
	children,
	className,
}: {
	open: boolean;
	onOpenChange: (v: boolean) => void;
	title: string;
	description?: React.ReactNode;
	submitLabel: string;
	action: (form: FormData) => Promise<R>;
	invalidate?: QueryKey[];
	success: string;
	onCreated?: (r: R) => void;
	successView?: (r: R) => React.ReactNode;
	children: React.ReactNode;
	className?: string;
}) {
	const qc = useQueryClient();
	const [error, setError] = React.useState<string | null>(null);
	const [busy, setBusy] = React.useState(false);
	const [result, setResult] = React.useState<{ r: R } | null>(null);
	// A fresh form each time it opens.
	const [generation, setGeneration] = React.useState(0);

	const change = (v: boolean) => {
		if (v) {
			setError(null);
			setResult(null);
			setGeneration((g) => g + 1);
		}
		onOpenChange(v);
	};

	async function submit(e: React.FormEvent<HTMLFormElement>) {
		e.preventDefault();
		if (busy) return;
		setBusy(true);
		setError(null);
		try {
			const r = await action(new FormData(e.currentTarget));
			for (const k of invalidate) qc.invalidateQueries({ queryKey: k });
			qc.invalidateQueries({ queryKey: keys.pick });
			toast.success(success);
			onCreated?.(r);
			if (successView) setResult({ r });
			else onOpenChange(false);
		} catch (err) {
			setError(err instanceof ApiError || err instanceof Error ? err.message : String(err));
		} finally {
			setBusy(false);
		}
	}

	return (
		<Dialog open={open} onOpenChange={change}>
			<DialogContent title={title} description={description} className={className}>
				{result && successView ? (
					<div className="grid gap-3">
						{successView(result.r)}
						<DialogFooter>
							<Button variant="primary" onClick={() => onOpenChange(false)}>
								Done
							</Button>
						</DialogFooter>
					</div>
				) : (
					<form key={generation} className="grid gap-3" onSubmit={submit} noValidate={false}>
						{children}
						{error && (
							<p role="alert" className="flex items-start gap-2 rounded border border-bad/40 bg-bad/10 px-3 py-2 text-[13px] text-bad">
								<AlertCircle className="mt-0.5 size-4 shrink-0" />
								{error}
							</p>
						)}
						<DialogFooter>
							<Button type="button" variant="ghost" onClick={() => onOpenChange(false)}>
								Cancel
							</Button>
							<Button type="submit" variant="primary" disabled={busy}>
								{busy ? 'Working…' : submitLabel}
							</Button>
						</DialogFooter>
					</form>
				)}
			</DialogContent>
		</Dialog>
	);
}

/** Highlights a just-created row for a moment: `rowClass(id)` on its `<Tr>`. */
export function useNewRow<K extends string | number>() {
	const [id, setId] = React.useState<K | null>(null);
	React.useEffect(() => {
		if (id === null) return;
		const t = window.setTimeout(() => setId(null), 2600);
		return () => window.clearTimeout(t);
	}, [id]);
	return { mark: setId, rowClass: (k: K) => (k === id ? 'dh-new' : undefined) };
}
