import * as React from 'react';
import { cn } from '@/lib/utils';

/** Dense table: 32px rows, 1px rules, sticky caps header. */
export function Table({ children, className }: { children: React.ReactNode; className?: string }) {
	return (
		<div className={cn('overflow-x-auto', className)}>
			<table className="w-full border-collapse text-[13px]">{children}</table>
		</div>
	);
}

export function Th({ children, className, ...props }: React.ThHTMLAttributes<HTMLTableCellElement>) {
	return (
		<th
			className={cn(
				'sticky top-0 z-[1] h-8 border-b border-line bg-surface-2 px-3 text-left align-middle text-[11px] font-semibold uppercase tracking-[0.08em] text-muted whitespace-nowrap',
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
		<tr className={cn('hover:bg-surface-2/60', className)} {...props}>
			{children}
		</tr>
	);
}
