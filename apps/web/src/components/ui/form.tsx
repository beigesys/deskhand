import * as React from 'react';
import { Checkbox as CheckboxPrimitive } from 'radix-ui';
import { Check, Minus } from 'lucide-react';
import { cn } from '@/lib/utils';

const field =
	'h-control w-full rounded border border-line-strong bg-surface px-2 text-[13px] text-fg placeholder:text-subtle focus-visible:outline-2 focus-visible:outline-focus focus-visible:outline-offset-0 disabled:opacity-60';

export const Input = React.forwardRef<HTMLInputElement, React.InputHTMLAttributes<HTMLInputElement>>(
	({ className, ...props }, ref) => <input ref={ref} className={cn(field, className)} {...props} />,
);
Input.displayName = 'Input';

export const Textarea = React.forwardRef<HTMLTextAreaElement, React.TextareaHTMLAttributes<HTMLTextAreaElement>>(
	({ className, ...props }, ref) => (
		<textarea ref={ref} className={cn(field, 'h-auto min-h-20 py-1.5', className)} {...props} />
	),
);
Textarea.displayName = 'Textarea';

/** A native select: dense, accessible and keyboard friendly as is. */
export const Select = React.forwardRef<HTMLSelectElement, React.SelectHTMLAttributes<HTMLSelectElement>>(
	({ className, ...props }, ref) => (
		<select ref={ref} className={cn(field, 'cursor-pointer pr-6', className)} {...props} />
	),
);
Select.displayName = 'Select';

export function Field({
	label,
	hint,
	children,
	className,
}: {
	label: string;
	hint?: React.ReactNode;
	children: React.ReactNode;
	className?: string;
}) {
	return (
		<label className={cn('grid gap-1', className)}>
			<span className="label-caps">{label}</span>
			{children}
			{hint && <span className="text-xs text-muted">{hint}</span>}
		</label>
	);
}

export function Checkbox({
	checked,
	onCheckedChange,
	className,
	...props
}: {
	checked: boolean | 'indeterminate';
	onCheckedChange: (v: boolean) => void;
	className?: string;
	'aria-label'?: string;
}) {
	return (
		<CheckboxPrimitive.Root
			checked={checked}
			onCheckedChange={(v) => onCheckedChange(v === true)}
			className={cn(
				'grid size-4 place-items-center rounded-sm border border-line-strong bg-surface data-[state=checked]:border-signal data-[state=checked]:bg-signal data-[state=indeterminate]:border-signal data-[state=indeterminate]:bg-signal text-signal-fg cursor-pointer',
				className,
			)}
			{...props}
		>
			<CheckboxPrimitive.Indicator>
				{checked === 'indeterminate' ? <Minus className="size-3" /> : <Check className="size-3" />}
			</CheckboxPrimitive.Indicator>
		</CheckboxPrimitive.Root>
	);
}
