import * as React from 'react';
import { Command } from 'cmdk';
import { Popover } from 'radix-ui';
import { useInfiniteQuery, useQuery } from '@tanstack/react-query';
import { Check, ChevronsUpDown, Loader2, X } from 'lucide-react';
import { api } from '@/api/client';
import type { Device, Group, Page, Script, User } from '@/api/types';
import { cn } from '@/lib/utils';

const PAGE = 25;

export interface Option {
	id: string;
	label: string;
	/** Second line or right-hand detail (an IP, a count). */
	detail?: string;
}

function useDebounced<T>(v: T, ms: number): T {
	const [d, setD] = React.useState(v);
	React.useEffect(() => {
		const t = setTimeout(() => setD(v), ms);
		return () => clearTimeout(t);
	}, [v, ms]);
	return d;
}

// Labels seen for each endpoint, so a selected id shows its name wherever it was loaded.
const labels = new Map<string, Map<string, Option>>();
function remember(endpoint: string, opts: Option[]) {
	let m = labels.get(endpoint);
	if (!m) labels.set(endpoint, (m = new Map()));
	for (const o of opts) m.set(o.id, o);
}

function recentKey(kind: string) {
	return `deskhand-recent:${kind}`;
}
function readRecent(kind: string): Option[] {
	try {
		return JSON.parse(localStorage.getItem(recentKey(kind)) ?? '[]') as Option[];
	} catch {
		return [];
	}
}
function pushRecent(kind: string, o: Option) {
	try {
		const list = [o, ...readRecent(kind).filter((r) => r.id !== o.id)].slice(0, 6);
		localStorage.setItem(recentKey(kind), JSON.stringify(list));
	} catch {
		/* no storage: no recents */
	}
}

interface BaseProps<T> {
	/** List endpoint under /api/v1, answering `?q=&cursor=&limit=&ids=` with a page. */
	endpoint: string;
	/** Extra fixed query parameters (a status filter, say). */
	params?: Record<string, string | undefined>;
	/** Turns an item into an option. */
	toOption: (item: T) => Option;
	/** Remembers recent picks under this name; defaults to the endpoint. */
	kind?: string;
	placeholder?: string;
	searchPlaceholder?: string;
	/** Options that don't come from the server, listed first (e.g. "No group"). */
	extra?: Option[];
	className?: string;
	disabled?: boolean;
	'aria-label'?: string;
	id?: string;
}

type SingleProps<T> = BaseProps<T> & { multiple?: false; value: string | null; onChange: (v: string | null, o: Option | null) => void };
type MultiProps<T> = BaseProps<T> & { multiple: true; value: string[]; onChange: (v: string[], o: Option[]) => void };

/**
 * A searchable picker over a server list that can grow without limit: debounced search
 * in SQL, 25 at a time with infinite scroll, recent picks first, and labels for selected
 * ids resolved even when they aren't on a loaded page.
 */
export function AsyncCombobox<T>(props: SingleProps<T> | MultiProps<T>) {
	const { endpoint, params, toOption, placeholder = 'Pick…', searchPlaceholder = 'Search…', extra = [] } = props;
	const kind = props.kind ?? endpoint;
	const [open, setOpen] = React.useState(false);
	const [search, setSearch] = React.useState('');
	const q = useDebounced(search.trim(), 200);
	const selected = props.multiple ? props.value : props.value ? [props.value] : [];
	const qs = (extraParams: Record<string, string | undefined>) => {
		const p = new URLSearchParams();
		for (const [k, v] of Object.entries({ ...params, ...extraParams })) if (v) p.set(k, v);
		return p.toString();
	};

	const list = useInfiniteQuery({
		queryKey: ['pick', endpoint, params ?? {}, q],
		queryFn: ({ pageParam }) => api.get<Page<T>>(`${endpoint}?${qs({ q, cursor: pageParam, limit: String(PAGE) })}`),
		initialPageParam: undefined as string | undefined,
		getNextPageParam: (p) => p.next_cursor ?? undefined,
		enabled: open,
		staleTime: 10_000,
	});
	const options = React.useMemo(() => {
		const o = (list.data?.pages ?? []).flatMap((p) => p.items.map(toOption));
		remember(endpoint, o);
		return o;
		// eslint-disable-next-line react-hooks/exhaustive-deps
	}, [list.data, endpoint]);

	// Selected ids we have no label for yet.
	const known = labels.get(endpoint);
	const missing = selected.filter((id) => !known?.has(id) && !extra.some((e) => e.id === id));
	const resolved = useQuery({
		queryKey: ['pick-ids', endpoint, missing.join(',')],
		queryFn: async () => {
			const page = await api.get<Page<T>>(`${endpoint}?${qs({ ids: missing.join(','), limit: String(Math.min(200, missing.length)) })}`);
			const o = page.items.map(toOption);
			remember(endpoint, o);
			return o;
		},
		enabled: missing.length > 0,
		staleTime: 60_000,
	});
	const optionOf = (id: string): Option =>
		extra.find((e) => e.id === id) ??
		labels.get(endpoint)?.get(id) ??
		resolved.data?.find((o) => o.id === id) ?? { id, label: resolved.isLoading ? '…' : id };

	const recents = !q ? readRecent(kind).filter((r) => !extra.some((e) => e.id === r.id)) : [];
	const shown = options.filter((o) => !recents.some((r) => r.id === o.id));

	function pick(o: Option) {
		pushRecent(kind, o);
		if (props.multiple) {
			const has = props.value.includes(o.id);
			const v = has ? props.value.filter((x) => x !== o.id) : [...props.value, o.id];
			props.onChange(v, v.map(optionOf));
		} else {
			props.onChange(o.id, o);
			setOpen(false);
		}
	}

	function onScroll(e: React.UIEvent<HTMLDivElement>) {
		const el = e.currentTarget;
		if (el.scrollTop + el.clientHeight > el.scrollHeight - 48 && list.hasNextPage && !list.isFetchingNextPage) {
			list.fetchNextPage();
		}
	}

	const item = (o: Option, group: string) => (
		<Command.Item
			key={`${group}:${o.id}`}
			value={`${group}:${o.id}`}
			onSelect={() => pick(o)}
			className="flex min-h-8 cursor-pointer items-center gap-2 px-2.5 py-1 text-[13px] outline-none data-[selected=true]:bg-chip max-md:min-h-10"
		>
			<Check className={cn('size-3.5 shrink-0 text-signal', !selected.includes(o.id) && 'invisible')} />
			<span className="min-w-0 flex-1 truncate">{o.label}</span>
			{o.detail && <span className="shrink-0 font-mono text-[11px] text-muted">{o.detail}</span>}
		</Command.Item>
	);

	const MAX_CHIPS = 3;
	return (
		<Popover.Root
			open={open}
			onOpenChange={(v) => {
				setOpen(v);
				if (!v) setSearch('');
			}}
		>
			<Popover.Trigger asChild disabled={props.disabled}>
				<button
					type="button"
					id={props.id}
					role="combobox"
					aria-expanded={open}
					aria-label={props['aria-label']}
					className={cn(
						'flex min-h-control w-full cursor-pointer items-center gap-1.5 rounded border border-line-strong bg-surface px-2 py-1 text-left text-[13px] disabled:opacity-60',
						props.className,
					)}
				>
					<span className="flex min-w-0 flex-1 flex-wrap items-center gap-1">
						{selected.length === 0 ? (
							<span className="text-subtle">{placeholder}</span>
						) : props.multiple ? (
							<>
								{selected.slice(0, MAX_CHIPS).map((id) => (
									<span key={id} className="chip gap-1 font-sans">
										{optionOf(id).label}
										<span
											role="button"
											tabIndex={-1}
											aria-label={`Remove ${optionOf(id).label}`}
											className="text-muted hover:text-bad"
											onPointerDown={(e) => {
												e.preventDefault();
												e.stopPropagation();
												const v = props.value.filter((x) => x !== id);
												props.onChange(v, v.map(optionOf));
											}}
										>
											<X className="size-3" />
										</span>
									</span>
								))}
								{selected.length > MAX_CHIPS && <span className="chip font-sans text-muted">+{selected.length - MAX_CHIPS} more</span>}
							</>
						) : (
							<span className="truncate">{optionOf(selected[0]).label}</span>
						)}
					</span>
					<ChevronsUpDown className="size-3.5 shrink-0 text-muted" />
				</button>
			</Popover.Trigger>
			<Popover.Portal>
				<Popover.Content
					align="start"
					sideOffset={4}
					className="z-50 w-[var(--radix-popover-trigger-width)] min-w-64 overflow-hidden rounded border border-line-strong bg-surface"
				>
					<Command shouldFilter={false} loop label={props['aria-label'] ?? placeholder}>
						<Command.Input
							value={search}
							onValueChange={setSearch}
							placeholder={searchPlaceholder}
							className="h-control w-full border-b border-line bg-transparent px-2.5 text-[13px] outline-none placeholder:text-subtle"
						/>
						<Command.List className="max-h-72 overflow-y-auto py-1" onScroll={onScroll}>
							{list.isLoading ? (
								<Command.Loading>
									<div className="flex items-center gap-2 px-3 py-3 text-[13px] text-muted">
										<Loader2 className="size-4 animate-spin" /> Loading…
									</div>
								</Command.Loading>
							) : list.isError ? (
								<div className="px-3 py-3 text-[13px] text-bad">
									Couldn't load: {String((list.error as Error).message)}{' '}
									<button className="underline" onClick={() => list.refetch()}>
										Retry
									</button>
								</div>
							) : (
								<>
									{extra.length > 0 && !q && <Command.Group>{extra.map((o) => item(o, 'extra'))}</Command.Group>}
									{recents.length > 0 && (
										<Command.Group heading="Recent" className="[&_[cmdk-group-heading]]:label-caps [&_[cmdk-group-heading]]:px-2.5 [&_[cmdk-group-heading]]:py-1">
											{recents.map((o) => item(o, 'recent'))}
										</Command.Group>
									)}
									{shown.length > 0 && (
										<Command.Group
											heading={recents.length ? 'All' : undefined}
											className="[&_[cmdk-group-heading]]:label-caps [&_[cmdk-group-heading]]:px-2.5 [&_[cmdk-group-heading]]:py-1"
										>
											{shown.map((o) => item(o, 'all'))}
										</Command.Group>
									)}
									{options.length === 0 && (
										<div className="px-3 py-3 text-[13px] text-muted">{q ? `Nothing matches “${q}”.` : 'Nothing here yet.'}</div>
									)}
									{list.isFetchingNextPage && (
										<div className="flex items-center gap-2 px-3 py-2 text-xs text-muted">
											<Loader2 className="size-3.5 animate-spin" /> Loading more…
										</div>
									)}
								</>
							)}
						</Command.List>
					</Command>
				</Popover.Content>
			</Popover.Portal>
		</Popover.Root>
	);
}

// --- ready-made pickers ---


export const deviceOption = (d: Device): Option => ({ id: d.id, label: d.name, detail: d.ip ?? d.os });
export const groupOption = (g: Group): Option => ({ id: String(g.id), label: g.name, detail: String(g.devices) });
export const scriptOption = (s: Script): Option => ({ id: String(s.id), label: s.name, detail: s.shell });
export const userOption = (u: User): Option => ({ id: String(u.id), label: u.username, detail: u.role });

export const NO_GROUP: Option = { id: 'none', label: 'No group' };
