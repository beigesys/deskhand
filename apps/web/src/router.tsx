import { createRootRoute, createRoute, createRouter, lazyRouteComponent, Outlet } from '@tanstack/react-router';
import { AppShell } from '@/components/layout/AppShell';
import { LoginPage, SetupPage } from '@/pages/auth';
import { DashboardPage } from '@/pages/dashboard';
import { DevicesPage } from '@/pages/devices';
import { DevicePage } from '@/pages/device';
import { JobsPage, JobPage } from '@/pages/jobs';
import { ScriptsPage } from '@/pages/scripts-list';
import { GroupsPage, EnrollPage, UsersPage, AuditPage } from '@/pages/admin';
import { StylePage } from '@/pages/style';
import { AccountPage } from '@/pages/account';
import { ConnectPage, McpPage } from '@/pages/mcp';
import { takeNext } from '@/lib/utils';
import * as React from 'react';

const str = (v: unknown) => (typeof v === 'string' && v ? v : undefined);

/** After SSO, which always lands on `/`: on to the page the sign-in started from. */
function Root() {
	React.useEffect(() => {
		if (window.location.pathname !== '/') return;
		const next = takeNext();
		if (next) window.location.replace(next);
	}, []);
	return <Outlet />;
}

const root = createRootRoute({ component: Root });

/** Where an MCP client sends the browser to be allowed (from `/oauth/authorize`). */
const connect = createRoute({ getParentRoute: () => root, path: '/connect', component: ConnectPage });

const login = createRoute({
	getParentRoute: () => root,
	path: '/login',
	validateSearch: (s: Record<string, unknown>): { next?: string; bad?: string } => ({ next: str(s.next), bad: str(s.bad) }),
	component: LoginPage,
});

const setup = createRoute({
	getParentRoute: () => root,
	path: '/setup',
	validateSearch: (s: Record<string, unknown>): { token?: string } => ({ token: str(s.token) }),
	component: SetupPage,
});

const app = createRoute({ getParentRoute: () => root, id: 'app', component: AppShell });

const dashboard = createRoute({ getParentRoute: () => app, path: '/', component: DashboardPage });

export interface DeviceSearch {
	q?: string;
	group?: string;
	status?: 'online' | 'offline' | 'attention';
	os?: string;
}

const devices = createRoute({
	getParentRoute: () => app,
	path: '/devices',
	validateSearch: (s: Record<string, unknown>): DeviceSearch => ({
		q: str(s.q),
		group: str(s.group),
		status: ['online', 'offline', 'attention'].includes(s.status as string) ? (s.status as DeviceSearch['status']) : undefined,
		os: str(s.os),
	}),
	component: DevicesPage,
});

const device = createRoute({
	getParentRoute: () => app,
	path: '/devices/$id',
	validateSearch: (s: Record<string, unknown>): { tab?: string } => ({ tab: str(s.tab) }),
	component: DevicePage,
});

const jobs = createRoute({ getParentRoute: () => app, path: '/jobs', component: JobsPage });
const job = createRoute({ getParentRoute: () => app, path: '/jobs/$id', component: JobPage });
const scripts = createRoute({ getParentRoute: () => app, path: '/scripts', component: ScriptsPage });
const script = createRoute({ getParentRoute: () => app, path: '/scripts/$id', component: lazyRouteComponent(() => import('@/pages/scripts'), 'ScriptPage') });
const groups = createRoute({ getParentRoute: () => app, path: '/groups', component: GroupsPage });
const enroll = createRoute({ getParentRoute: () => app, path: '/enroll', component: EnrollPage });
const users = createRoute({ getParentRoute: () => app, path: '/users', component: UsersPage });
const audit = createRoute({ getParentRoute: () => app, path: '/audit', component: AuditPage });
const mcp = createRoute({ getParentRoute: () => app, path: '/mcp-clients', component: McpPage });
const style = createRoute({ getParentRoute: () => app, path: '/style', component: StylePage });
const account = createRoute({ getParentRoute: () => app, path: '/account', component: AccountPage });

const tree = root.addChildren([
	login,
	setup,
	connect,
	app.addChildren([dashboard, devices, device, jobs, job, scripts, script, groups, enroll, users, audit, mcp, style, account]),
]);

export const router = createRouter({ routeTree: tree, defaultPreload: 'intent' });

declare module '@tanstack/react-router' {
	interface Register {
		router: typeof router;
	}
}
