import * as React from 'react';
import { Button } from './button';
import { Dialog, DialogContent, DialogFooter } from './overlay';
import { Field, Input } from './form';

/**
 * A confirm dialog. With `typed`, the user has to type that text (a hostname, a count)
 * before the action is enabled; `typed` is passed to `onConfirm`.
 */
export function Confirm({
	open,
	onOpenChange,
	title,
	body,
	action,
	danger,
	typed,
	typedLabel,
	busy,
	onConfirm,
}: {
	open: boolean;
	onOpenChange: (v: boolean) => void;
	title: string;
	body?: React.ReactNode;
	action: string;
	danger?: boolean;
	typed?: string;
	typedLabel?: string;
	busy?: boolean;
	onConfirm: (typed: string) => void;
}) {
	const [text, setText] = React.useState('');
	React.useEffect(() => {
		if (open) setText('');
	}, [open]);
	const ok = !typed || text.trim().toLowerCase() === typed.toLowerCase();
	return (
		<Dialog open={open} onOpenChange={onOpenChange}>
			<DialogContent title={title}>
				<form
					onSubmit={(e) => {
						e.preventDefault();
						if (ok) onConfirm(text.trim());
					}}
				>
					{body && <div className="text-[13px] text-muted">{body}</div>}
					{typed && (
						<Field
							className="mt-3"
							label={typedLabel ?? 'Type to confirm'}
							hint={
								<>
									Type <code className="chip">{typed}</code> to confirm.
								</>
							}
						>
							<Input autoFocus value={text} onChange={(e) => setText(e.target.value)} autoComplete="off" />
						</Field>
					)}
					<DialogFooter>
						<Button variant="ghost" onClick={() => onOpenChange(false)}>
							Cancel
						</Button>
						<Button type="submit" variant={danger ? 'danger' : 'primary'} disabled={!ok || busy}>
							{action}
						</Button>
					</DialogFooter>
				</form>
			</DialogContent>
		</Dialog>
	);
}
