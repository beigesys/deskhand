import * as React from 'react';
import { cn } from '@/lib/utils';

/** Dense table: 32px rows, 1px rules, sticky caps header. */
export function Table({ children, className }: { children: React.ReactNode; className?: string }) {
	return (
		<div className={cn('overflow-x-auto', className)}>
			<table className="dh-table w-full border-collapse text-[13px]">{children}</table>
		</div>
	);
}

export function Th({ children, className, ...props }: React.ThHTMLAttributes<HTMLTableCellElement>) {
	return (
		<th
			className={cn(
				'h-8 border-b border-line bg-surface px-3 text-left align-middle text-[10.5px] font-medium uppercase tracking-[1.2px] text-muted whitespace-nowrap',
				className,
			)}
			{...props}
		>
			{children}
		</th>
	);
}

export function Td({ children, className, ...props }: React.TdHTMLAttributes<HTMLTableCellElement>) {
	return (
		<td className={cn('h-row border-b border-line px-3 align-middle whitespace-nowrap', className)} {...props}>
			{children}
		</td>
	);
}

export function Tr({ children, className, ...props }: React.HTMLAttributes<HTMLTableRowElement>) {
	return (
		<tr className={cn('[&:last-child>td]:border-b-0 hover:[&>td]:bg-signal/5', className)} {...props}>
			{children}
		</tr>
	);
}
