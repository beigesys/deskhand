import * as React from 'react';
import { Dialog as D, DropdownMenu as M, Tabs as T, Tooltip as TT } from 'radix-ui';
import { X } from 'lucide-react';
import { cn } from '@/lib/utils';

// --- dialog ---

export const Dialog = D.Root;
export const DialogTrigger = D.Trigger;
export const DialogClose = D.Close;

export function DialogContent({
	title,
	description,
	children,
	className,
}: {
	title: string;
	description?: React.ReactNode;
	children: React.ReactNode;
	className?: string;
}) {
	return (
		<D.Portal>
			<D.Overlay className="fixed inset-0 z-40 bg-navy-950/60" />
			<D.Content
				className={cn(
					'fixed left-1/2 top-[10vh] z-50 grid max-h-[80vh] w-[calc(100vw-32px)] max-w-lg -translate-x-1/2 gap-0 overflow-hidden rounded border border-line-strong bg-surface',
					className,
				)}
			>
				<div className="flex items-start justify-between gap-4 border-b border-line bg-surface-2 px-4 py-2.5">
					<div>
						<D.Title className="text-[15px] font-semibold">{title}</D.Title>
						{description ? (
							<D.Description className="mt-0.5 text-xs text-muted">{description}</D.Description>
						) : (
							<D.Description className="sr-only">{title}</D.Description>
						)}
					</div>
					<D.Close className="text-muted hover:text-fg" aria-label="Close">
						<X className="size-4" />
					</D.Close>
				</div>
				<div className="overflow-y-auto p-4">{children}</div>
			</D.Content>
		</D.Portal>
	);
}

export function DialogFooter({ children }: { children: React.ReactNode }) {
	return <div className="mt-4 flex flex-wrap justify-end gap-2">{children}</div>;
}

// --- menu ---

export const Menu = M.Root;
export const MenuTrigger = M.Trigger;

export function MenuContent({ children, align = 'end' }: { children: React.ReactNode; align?: 'start' | 'end' }) {
	return (
		<M.Portal>
			<M.Content
				align={align}
				sideOffset={4}
				className="z-50 min-w-44 rounded border border-line-strong bg-surface py-1 text-[13px]"
			>
				{children}
			</M.Content>
		</M.Portal>
	);
}

export function MenuItem({
	children,
	onSelect,
	danger,
	disabled,
}: {
	children: React.ReactNode;
	onSelect?: () => void;
	danger?: boolean;
	disabled?: boolean;
}) {
	return (
		<M.Item
			onSelect={onSelect}
			disabled={disabled}
			className={cn(
				'flex h-7 cursor-pointer items-center gap-2 px-3 outline-none data-[highlighted]:bg-surface-2 data-[disabled]:opacity-50 [&_svg]:size-4',
				danger && 'text-bad',
			)}
		>
			{children}
		</M.Item>
	);
}

export const MenuSeparator = () => <M.Separator className="my-1 h-px bg-line" />;
export const MenuLabel = ({ children }: { children: React.ReactNode }) => (
	<M.Label className="label-caps px-3 py-1">{children}</M.Label>
);

// --- tabs ---

export const Tabs = T.Root;
export const TabsContent = T.Content;

export function TabsList({ children }: { children: React.ReactNode }) {
	return <T.List className="flex gap-0 overflow-x-auto border-b border-line">{children}</T.List>;
}

export function TabsTrigger({ value, children }: { value: string; children: React.ReactNode }) {
	return (
		<T.Trigger
			value={value}
			className="-mb-px h-9 cursor-pointer whitespace-nowrap border-b-2 border-transparent px-3 text-[13px] font-medium text-muted hover:text-fg data-[state=active]:border-signal data-[state=active]:text-fg"
		>
			{children}
		</T.Trigger>
	);
}

// --- tooltip ---

export const TooltipProvider = TT.Provider;

export function Tip({ label, children }: { label: React.ReactNode; children: React.ReactNode }) {
	return (
		<TT.Root delayDuration={300}>
			<TT.Trigger asChild>{children}</TT.Trigger>
			<TT.Portal>
				<TT.Content
					sideOffset={4}
					className="z-50 rounded-sm bg-navy-900 px-2 py-1 text-xs text-white"
				>
					{label}
				</TT.Content>
			</TT.Portal>
		</TT.Root>
	);
}
