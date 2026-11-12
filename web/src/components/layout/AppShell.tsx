import * as React from 'react';
import { Link, Outlet, useNavigate, useRouterState } from '@tanstack/react-router';
import { useQueryClient } from '@tanstack/react-query';
import {
	Anchor,
	Gauge,
	LogOut,
	Menu as MenuIcon,
	Monitor,
	Moon,
	Palette,
	PlayCircle,
	ScrollText,
	Sun,
	SunMoon,
	Ticket,
	Users,
	FolderTree,
	FileCode2,
	X,
	ExternalLink,
} from 'lucide-react';
import { ApiError } from '@/api/client';
import { api } from '@/api/client';
import { useCan, useDeviceEvents, useMe } from '@/api/hooks';
import type { Role } from '@/api/types';
import { cn } from '@/lib/utils';
import { Menu, MenuContent, MenuItem, MenuLabel, MenuSeparator, MenuTrigger } from '@/components/ui/overlay';
import { useTheme } from '@/lib/theme';

const nav: { to: string; label: string; icon: React.ElementType; min: Role; exact?: boolean }[] = [
	{ to: '/', label: 'Dashboard', icon: Gauge, min: 'viewer', exact: true },
	{ to: '/devices', label: 'Devices', icon: Monitor, min: 'viewer' },
	{ to: '/jobs', label: 'Jobs', icon: PlayCircle, min: 'viewer' },
	{ to: '/scripts', label: 'Scripts', icon: FileCode2, min: 'viewer' },
	{ to: '/groups', label: 'Groups', icon: FolderTree, min: 'admin' },
	{ to: '/enroll', label: 'Enrollment', icon: Ticket, min: 'admin' },
	{ to: '/users', label: 'Users', icon: Users, min: 'admin' },
	{ to: '/audit', label: 'Audit', icon: ScrollText, min: 'admin' },
];

export function Logo({ className }: { className?: string }) {
	return (
		<span className={cn('flex items-center gap-2 font-semibold tracking-tight', className)}>
			<span className="grid size-6 place-items-center rounded-sm bg-signal text-white">
				<Anchor className="size-4" />
			</span>
			Deskhand
		</span>
	);
}

function Sidebar({ onNavigate }: { onNavigate?: () => void }) {
	const isAdmin = useCan('admin');
	const can = { viewer: true, operator: useCan('operator'), admin: isAdmin };
	const path = useRouterState({ select: (s) => s.location.pathname });
	return (
		<nav className="flex h-full flex-col bg-side text-side-fg" aria-label="Main">
			<div className="flex h-12 items-center border-b border-side-line px-4 text-white">
				<Logo />
			</div>
			<div className="flex-1 overflow-y-auto py-2">
				<div className="label-caps px-4 pb-1 pt-2 !text-side-muted">Fleet</div>
				{nav
					.filter((n) => can[n.min])
					.map((n, i) => {
						const active = n.exact ? path === n.to : path === n.to || path.startsWith(`${n.to}/`);
						return (
							<React.Fragment key={n.to}>
								{i === 4 && <div className="label-caps px-4 pb-1 pt-4 !text-side-muted">Admin</div>}
								<Link
									to={n.to}
									onClick={onNavigate}
									className={cn(
										'mx-2 flex h-8 items-center gap-2.5 rounded px-2.5 text-[13px] hover:bg-side-active/60 hover:text-white',
										active && 'bg-side-active text-side-active-fg shadow-[inset_2px_0_0_var(--dh-signal)]',
									)}
								>
									<n.icon className="size-4" />
									{n.label}
								</Link>
							</React.Fragment>
						);
					})}
			</div>
			<div className="border-t border-side-line p-2 text-xs">
				{isAdmin && (
					<Link
						to="/style"
						onClick={onNavigate}
						className="flex h-7 items-center gap-2 rounded px-2.5 text-side-muted hover:text-white"
					>
						<Palette className="size-3.5" /> Style guide
					</Link>
				)}
				<a href="/classic" className="flex h-7 items-center gap-2 rounded px-2.5 text-side-muted hover:text-white">
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
	if (!me) return null;
	return (
		<Menu>
			<MenuTrigger className="flex h-8 cursor-pointer items-center gap-2 rounded px-2 text-[13px] hover:bg-surface-2">
				<span className="grid size-6 place-items-center rounded-sm bg-navy-800 font-mono text-[11px] uppercase text-white">
					{me.username.slice(0, 2)}
				</span>
				<span className="hidden sm:inline">{me.username}</span>
				<span className="chip hidden sm:inline-flex">{me.role}</span>
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
	const path = useRouterState({ select: (s) => s.location.pathname });
	useDeviceEvents(!!me.data);

	React.useEffect(() => {
		if (me.error instanceof ApiError && me.error.status === 401) {
			navigate({ to: '/login', search: { next: path === '/' ? undefined : path } });
		}
	}, [me.error, navigate, path]);

	if (!me.data) {
		return <div className="grid min-h-dvh place-items-center text-muted">{me.isError ? 'Redirecting…' : 'Loading…'}</div>;
	}

	return (
		<div className="flex min-h-dvh">
			<aside className="sticky top-0 hidden h-dvh w-56 shrink-0 md:block">
				<Sidebar />
			</aside>
			{drawer && (
				<div className="fixed inset-0 z-40 md:hidden">
					<div className="absolute inset-0 bg-navy-950/60" onClick={() => setDrawer(false)} />
					<div className="absolute inset-y-0 left-0 w-64">
						<Sidebar onNavigate={() => setDrawer(false)} />
						<button
							className="absolute right-2 top-3 text-side-fg"
							onClick={() => setDrawer(false)}
							aria-label="Close menu"
						>
							<X className="size-5" />
						</button>
					</div>
				</div>
			)}
			<div className="flex min-w-0 flex-1 flex-col">
				<header className="sticky top-0 z-30 flex h-12 items-center gap-2 border-b border-line bg-surface px-3 md:px-5">
					<button className="md:hidden" onClick={() => setDrawer(true)} aria-label="Open menu">
						<MenuIcon className="size-5" />
					</button>
					<Logo className="md:hidden" />
					<div className="flex-1" />
					<UserMenu />
				</header>
				<main className="min-w-0 flex-1 p-3 md:p-5">
					<Outlet />
				</main>
			</div>
		</div>
	);
}
