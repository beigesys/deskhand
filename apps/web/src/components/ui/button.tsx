import * as React from 'react';
import { Slot } from 'radix-ui';
import { cva, type VariantProps } from 'class-variance-authority';
import { cn } from '@/lib/utils';

export const buttonVariants = cva(
	'inline-flex items-center justify-center gap-1.5 whitespace-nowrap rounded border text-[13px] font-medium transition-colors motion-reduce:transition-none disabled:pointer-events-none disabled:opacity-50 [&_svg]:size-4 [&_svg]:shrink-0 cursor-pointer select-none',
	{
		variants: {
			variant: {
				primary: 'border-signal bg-signal text-signal-ink hover:bg-signal-hover hover:border-signal-hover',
				secondary: 'border-line bg-surface text-fg hover:border-muted',
				ghost: 'border-transparent bg-transparent text-muted hover:bg-surface-2 hover:text-fg',
				danger: 'border-bad/50 bg-surface text-bad hover:bg-bad hover:text-white hover:border-bad',
				/** Small outline action in table rows (Desktop, Shell). */
				act: 'border-line bg-transparent text-fg hover:border-signal hover:text-signal',
				dark: 'border-rail bg-rail text-rail-strong hover:bg-rail-active',
			},
			size: {
				sm: 'h-6 px-2 text-xs max-md:h-10 max-md:px-3',
				md: 'h-control px-3',
				lg: 'h-9 px-4 text-sm',
				icon: 'size-control p-0',
			},
		},
		defaultVariants: { variant: 'secondary', size: 'md' },
	},
);

export interface ButtonProps
	extends React.ButtonHTMLAttributes<HTMLButtonElement>,
		VariantProps<typeof buttonVariants> {
	asChild?: boolean;
}

export const Button = React.forwardRef<HTMLButtonElement, ButtonProps>(
	({ className, variant, size, asChild, type, ...props }, ref) => {
		const Comp = asChild ? Slot.Root : 'button';
		return (
			<Comp
				ref={ref}
				type={asChild ? undefined : (type ?? 'button')}
				className={cn(buttonVariants({ variant, size }), className)}
				{...props}
			/>
		);
	},
);
Button.displayName = 'Button';
