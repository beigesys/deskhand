import * as React from 'react';
import { Link, Outlet, useNavigate, useRouterState } from '@tanstack/react-router';
import { useQueryClient } from '@tanstack/react-query';
import {
	ExternalLink,
	FileCode2,
	FolderTree,
	Gauge,
	LogOut,
	Menu as MenuIcon,
	Monitor,
	Moon,
	Palette as PaletteIcon,
	PlayCircle,
	ScrollText,
	Sun,
	SunMoon,
	Ticket,
	Users,
} from 'lucide-react';
import { ApiError, api } from '@/api/client';
import { useCan, useDeviceEvents, useJobs, useMe, useSummary } from '@/api/hooks';
import type { Role } from '@/api/types';
import { cn } from '@/lib/utils';
import { TopbarSlot } from '@/components/ui/display';
import { Menu, MenuContent, MenuItem, MenuLabel, MenuSeparator, MenuTrigger, Sheet } from '@/components/ui/overlay';
import { PALETTES, usePalette, useTheme } from '@/lib/theme';

const nav: { to: string; label: string; icon: React.ElementType; min: Role; exact?: boolean; count?: 'devices' | 'jobs' }[] = [
	{ to: '/', label: 'Overview', icon: Gauge, min: 'viewer', exact: true },
	{ to: '/devices', label: 'Devices', icon: Monitor, min: 'viewer', count: 'devices' },
	{ to: '/jobs', label: 'Jobs', icon: PlayCircle, min: 'viewer', count: 'jobs' },
	{ to: '/scripts', label: 'Scripts', icon: FileCode2, min: 'viewer' },
	{ to: '/groups', label: 'Groups', icon: FolderTree, min: 'admin' },
	{ to: '/enroll', label: 'Enrollment', icon: Ticket, min: 'admin' },
	{ to: '/users', label: 'Users', icon: Users, min: 'admin' },
	{ to: '/audit', label: 'Audit', icon: ScrollText, min: 'admin' },
];

/** The mark: a screen with a signal-orange frame. */
function Mark() {
	return (
		<svg width="22" height="22" viewBox="0 0 24 24" aria-hidden="true" className="shrink-0">
			<rect x="2" y="4" width="20" height="13" rx="2" fill="none" stroke="var(--dh-signal)" strokeWidth="1.8" />
			<path d="M8 21h8M12 17v4" stroke="var(--dh-signal)" strokeWidth="1.8" strokeLinecap="round" />
			<path d="M6 13l3-4 3 3 2-2 4 3" fill="none" stroke="var(--dh-rail-fg)" strokeWidth="1.5" strokeLinejoin="round" />
		</svg>
	);
}

export function Logo({ className }: { className?: string }) {
	return (
		<span className={cn('flex items-center gap-2.5 text-base font-semibold tracking-[0.2px]', className)}>
			<Mark />
			Deskhand
		</span>
	);
}

function Rail({ onNavigate }: { onNavigate?: () => void }) {
	const me = useMe();
	const isAdmin = useCan('admin');
	const can = { viewer: true, operator: useCan('operator'), admin: isAdmin };
	const path = useRouterState({ select: (s) => s.location.pathname });
	const summary = useSummary();
	const jobs = useJobs(undefined, { limit: 25 });
	const counts = {
		devices: summary.data?.total,
		jobs: jobs.items.reduce((n, j) => n + (j.counts ? j.counts.running + j.counts.queued : 0), 0) || undefined,
	};
	return (
		<nav
			className="flex h-full flex-col gap-1 overflow-y-auto border-r border-rail-border bg-rail px-3 py-4 text-rail-fg"
			aria-label="Main"
		>
			<Logo className="px-2 pb-4 text-rail-strong" />
			{nav
				.filter((n) => can[n.min])
				.map((n, i) => {
					const active = n.exact ? path === n.to : path === n.to || path.startsWith(`${n.to}/`);
					const count = n.count && counts[n.count];
					return (
						<React.Fragment key={n.to}>
							{i === 0 && <div className="px-2 pb-1.5 pt-1 text-[10.5px] uppercase tracking-[1.2px] text-rail-dim">Fleet</div>}
							{i === 4 && <div className="px-2 pb-1.5 pt-3.5 text-[10.5px] uppercase tracking-[1.2px] text-rail-dim">Admin</div>}
							<Link
								to={n.to}
								onClick={onNavigate}
								className={cn(
									'flex h-[34px] shrink-0 items-center gap-2.5 rounded px-2 text-[14px] transition-colors hover:bg-rail-hover motion-reduce:transition-none max-md:h-11',
									active && 'bg-rail-active text-rail-strong shadow-[inset_2px_0_0_var(--dh-signal)]',
								)}
								aria-current={active ? 'page' : undefined}
							>
								<n.icon className="size-4" />
								<span>{n.label}</span>
								{count ? <em className="ml-auto font-mono text-[11px] not-italic text-rail-dim">{count}</em> : null}
							</Link>
						</React.Fragment>
					);
				})}
			<div className="mt-auto grid shrink-0 gap-0.5 border-t border-rail-active px-2 pt-3 text-[12.5px] text-rail-dim">
				{me.data && (
					<div className="pb-2">
						<b className="block font-medium text-rail-fg">{me.data.username}</b>
						<span className="capitalize">{me.data.role}</span>
					</div>
				)}
				{isAdmin && (
					<Link to="/style" onClick={onNavigate} className="flex items-center gap-2 py-1 hover:text-rail-fg max-md:h-10">
						<PaletteIcon className="size-3.5" /> Style guide
					</Link>
				)}
				<a href="/classic" className="flex items-center gap-2 py-1 hover:text-rail-fg max-md:h-10">
					<ExternalLink className="size-3.5" /> Classic pages
				</a>
			</div>
		</nav>
	);
}

function UserMenu() {
	const { data: me } = useMe();
	const qc = useQueryClient();
	const navigate = useNavigate();
	const { theme, setTheme } = useTheme();
	const { palette, setPalette } = usePalette(me?.username);
	if (!me) return null;
	return (
		<Menu>
			<MenuTrigger
				className="flex h-control cursor-pointer items-center gap-2 rounded px-1.5 text-[13px] hover:bg-surface-2"
				aria-label="User settings"
			>
				<span className="grid size-7 place-items-center rounded-sm bg-rail font-mono text-[11px] uppercase text-rail-strong">
					{me.username.slice(0, 2)}
				</span>
				<span className="hidden lg:inline">{me.username}</span>
			</MenuTrigger>
			<MenuContent>
				<MenuLabel>Theme</MenuLabel>
				{(
					[
						['system', 'System', SunMoon],
						['light', 'Light', Sun],
						['dark', 'Dark', Moon],
					] as const
				).map(([v, label, Icon]) => (
					<MenuItem key={v} onSelect={() => setTheme(v)}>
						<Icon /> {label} {theme === v && <span className="ml-auto text-signal">●</span>}
					</MenuItem>
				))}
				<MenuSeparator />
				<MenuLabel>Palette</MenuLabel>
				{PALETTES.map((p) => (
					<MenuItem key={p.id} onSelect={() => setPalette(p.id)}>
						<i className="inline-block size-3 rounded-sm border border-line-strong" style={{ background: p.swatch }} />
						{p.label}
						{palette === p.id && <span className="ml-auto text-signal">●</span>}
					</MenuItem>
				))}
				<MenuSeparator />
				<MenuItem
					onSelect={async () => {
						await api.post('/logout').catch(() => {});
						qc.clear();
						navigate({ to: '/login' });
					}}
				>
					<LogOut /> Sign out
				</MenuItem>
			</MenuContent>
		</Menu>
	);
}

export function AppShell() {
	const me = useMe();
	const navigate = useNavigate();
	const [drawer, setDrawer] = React.useState(false);
	const [slot, setSlot] = React.useState<HTMLElement | null>(null);
	const path = useRouterState({ select: (s) => s.location.pathname });
	usePalette(me.data?.username);
	useDeviceEvents(!!me.data);

	// Signed out: to the sign-in page, coming back here after. Only from this layout's own
	// pages; the effect can still run once while the router is already leaving for /login.
	const unauthorized = me.error instanceof ApiError && me.error.status === 401;
	React.useEffect(() => {
		if (unauthorized && !path.startsWith('/login') && !path.startsWith('/setup')) {
			navigate({ to: '/login', search: { next: path === '/' ? undefined : path }, replace: true });
		}
	}, [unauthorized, navigate, path]);

	// The drawer closes on navigation.
	React.useEffect(() => setDrawer(false), [path]);

	if (!me.data) {
		return <div className="grid min-h-dvh place-items-center text-muted">{me.isError ? 'Redirecting…' : 'Loading…'}</div>;
	}

	return (
		<div className="min-h-dvh md:grid md:grid-cols-[216px_minmax(0,1fr)]">
			<aside className="sticky top-0 hidden h-dvh md:block">
				<Rail />
			</aside>
			<Sheet open={drawer} onOpenChange={setDrawer} title="Navigation">
				<Rail onNavigate={() => setDrawer(false)} />
			</Sheet>
			<div className="flex min-w-0 flex-col">
				<header className="sticky top-0 z-30 flex h-14 items-center gap-2 border-b border-line bg-surface px-2 md:h-12 md:justify-end md:px-6">
					<button
						className="grid size-10 place-items-center rounded hover:bg-surface-2 md:hidden"
						onClick={() => setDrawer(true)}
						aria-label="Open menu"
					>
						<MenuIcon className="size-5" />
					</button>
					<Logo className="gap-2 text-[15px] md:hidden" />
					<span className="h-5 w-px bg-line md:hidden" aria-hidden="true" />
					<div ref={setSlot} className="flex min-w-0 flex-1 items-center md:hidden" />
					<UserMenu />
				</header>
				<main className="min-w-0 flex-1 px-4 pb-8 pt-5 md:px-6">
					<TopbarSlot.Provider value={slot}>
						<Outlet />
					</TopbarSlot.Provider>
				</main>
			</div>
		</div>
	);
}
