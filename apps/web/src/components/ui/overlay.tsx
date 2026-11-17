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
			<D.Overlay className="fixed inset-0 z-40 bg-scrim" />
			<D.Content
				className={cn(
					// Desktop: a centered dialog. Phones: a full-height sheet that rises from the bottom.
					'fixed z-50 flex flex-col overflow-hidden border border-line-strong bg-surface outline-none',
					'md:left-1/2 md:top-[10vh] md:max-h-[80vh] md:w-[calc(100vw-32px)] md:max-w-lg md:-translate-x-1/2 md:rounded',
					'max-md:inset-x-0 max-md:bottom-0 max-md:top-[env(safe-area-inset-top,0px)] max-md:rounded-t-lg max-md:border-x-0 max-md:border-b-0',
					'max-md:data-[state=open]:animate-[dh-rise_.2s_ease-out] motion-reduce:animate-none',
					className,
				)}
			>
				<div className="flex shrink-0 items-start justify-between gap-4 border-b border-line bg-surface-2 px-4 py-2.5 max-md:py-3">
					<div>
						<D.Title className="text-[15px] font-semibold">{title}</D.Title>
						{description ? (
							<D.Description className="mt-0.5 text-xs text-muted">{description}</D.Description>
						) : (
							<D.Description className="sr-only">{title}</D.Description>
						)}
					</div>
					<D.Close className="grid place-items-center text-muted hover:text-fg max-md:size-10 max-md:-m-2" aria-label="Close">
						<X className="size-4 max-md:size-5" />
					</D.Close>
				</div>
				<div className="min-h-0 flex-1 overflow-y-auto p-4">{children}</div>
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
				'flex h-7 max-md:h-10 cursor-pointer items-center gap-2 px-3 outline-none data-[highlighted]:bg-surface-2 data-[disabled]:opacity-50 [&_svg]:size-4',
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
			className="-mb-px h-9 max-md:h-11 cursor-pointer focus-visible:outline-offset-[-2px] whitespace-nowrap border-b-2 border-transparent px-3 text-[13px] font-medium text-muted hover:text-fg data-[state=active]:border-signal data-[state=active]:text-fg"
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
					className="z-50 rounded-sm bg-rail px-2 py-1 text-xs text-rail-strong"
				>
					{label}
				</TT.Content>
			</TT.Portal>
		</TT.Root>
	);
}

// --- sheet: a dialog that slides in from the left (the phone navigation drawer) ---

export function Sheet({
	open,
	onOpenChange,
	title,
	children,
}: {
	open: boolean;
	onOpenChange: (v: boolean) => void;
	title: string;
	children: React.ReactNode;
}) {
	return (
		<D.Root open={open} onOpenChange={onOpenChange}>
			<D.Portal>
				<D.Overlay className="fixed inset-0 z-40 bg-scrim data-[state=open]:animate-[dh-fade_.15s_ease-out] motion-reduce:animate-none" />
				<D.Content
					className="fixed inset-y-0 left-0 z-50 flex w-[min(300px,85vw)] flex-col outline-none data-[state=open]:animate-[dh-slide_.18s_ease-out] motion-reduce:animate-none"
					aria-describedby={undefined}
				>
					<D.Title className="sr-only">{title}</D.Title>
					{children}
					<D.Close
						className="absolute right-2 top-2 grid size-10 place-items-center rounded text-rail-fg hover:bg-rail-hover"
						aria-label="Close menu"
					>
						<X className="size-5" />
					</D.Close>
				</D.Content>
			</D.Portal>
		</D.Root>
	);
}
