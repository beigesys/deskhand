import * as React from 'react';
import { Checkbox as CheckboxPrimitive, Select as SelectPrimitive } from 'radix-ui';
import { Check, ChevronDown, Minus } from 'lucide-react';
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

export interface SelectOption<T extends string> {
	value: T;
	label: React.ReactNode;
}

// Radix reserves '' for "nothing picked"; '' is a real value here ("any").
const EMPTY = '__empty__';

/**
 * Pick one of a small fixed set (a status, an OS, a shell); anything that can grow is an
 * AsyncCombobox. `prefix` labels the trigger ("OS: Windows") so a filter bar reads on its own.
 * Controlled with `value`/`onChange`, or with `name`/`defaultValue` inside a form.
 */
export function Select<T extends string>({
	value,
	defaultValue,
	onChange,
	name,
	options,
	prefix,
	className,
	disabled,
	id,
	...props
}: {
	value?: T;
	defaultValue?: T;
	onChange?: (v: T) => void;
	name?: string;
	options: SelectOption<T>[];
	prefix?: string;
	className?: string;
	disabled?: boolean;
	id?: string;
	'aria-label'?: string;
}) {
	const enc = (v: T | undefined) => (v === '' ? EMPTY : v);
	return (
		<SelectPrimitive.Root
			value={enc(value)}
			defaultValue={enc(defaultValue)}
			onValueChange={(v) => onChange?.((v === EMPTY ? '' : v) as T)}
			name={name}
			disabled={disabled}
		>
			<SelectPrimitive.Trigger
				id={id}
				aria-label={props['aria-label']}
				className={cn(field, 'inline-flex cursor-pointer items-center justify-between gap-2 text-left', className)}
			>
				<span className="min-w-0 truncate">
					{prefix && <span className="text-muted">{prefix}: </span>}
					<SelectPrimitive.Value />
				</span>
				<SelectPrimitive.Icon>
					<ChevronDown className="size-3.5 shrink-0 text-muted" />
				</SelectPrimitive.Icon>
			</SelectPrimitive.Trigger>
			<SelectPrimitive.Portal>
				<SelectPrimitive.Content
					position="popper"
					sideOffset={4}
					className="z-50 max-h-[var(--radix-select-content-available-height)] min-w-[var(--radix-select-trigger-width)] overflow-hidden rounded border border-line-strong bg-surface"
				>
					<SelectPrimitive.Viewport className="py-1">
						{options.map((o) => (
							<SelectPrimitive.Item
								key={o.value}
								value={o.value === '' ? EMPTY : o.value}
								className="flex min-h-8 cursor-pointer select-none items-center gap-2 px-2.5 py-1 text-[13px] outline-none data-[highlighted]:bg-chip max-md:min-h-10"
							>
								<span className="grid size-3.5 shrink-0 place-items-center">
									<SelectPrimitive.ItemIndicator>
										<Check className="size-3.5 text-signal" />
									</SelectPrimitive.ItemIndicator>
								</span>
								<SelectPrimitive.ItemText>{o.label}</SelectPrimitive.ItemText>
							</SelectPrimitive.Item>
						))}
					</SelectPrimitive.Viewport>
				</SelectPrimitive.Content>
			</SelectPrimitive.Portal>
		</SelectPrimitive.Root>
	);
}

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
