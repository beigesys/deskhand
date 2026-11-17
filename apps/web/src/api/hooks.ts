import { useEffect } from 'react';
import {
	useInfiniteQuery,
	useMutation,
	useQuery,
	useQueryClient,
	type InfiniteData,
	type QueryKey,
} from '@tanstack/react-query';
import { toast } from 'sonner';
import { api, ApiError } from './client';
import type {
	AgentDownloads,
	AuditRow,
	AuthInfo,
	Device,
	DeviceDetail,
	DeviceEvent,
	EnrollToken,
	Group,
	Job,
	JobDetail,
	Me,
	Page,
	Role,
	Run,
	Script,
	Summary,
	User,
} from './types';

export const keys = {
	me: ['me'] as const,
	auth: ['auth'] as const,
	devices: ['devices'] as const,
	summary: ['summary'] as const,
	device: (id: string) => ['device', id] as const,
	groups: ['groups'] as const,
	tokens: ['enroll-tokens'] as const,
	agentDownloads: ['agent-downloads'] as const,
	users: ['users'] as const,
	audit: ['audit'] as const,
	scripts: ['scripts'] as const,
	script: (id: number) => ['script', id] as const,
	jobs: ['jobs'] as const,
	job: (id: number) => ['job', id] as const,
	runs: (id: number) => ['runs', id] as const,
	pick: ['pick'] as const,
};

export function useMe() {
	return useQuery({
		queryKey: keys.me,
		queryFn: () => api.get<Me>('/me'),
		retry: false,
		staleTime: 60_000,
	});
}

const rank: Record<Role, number> = { viewer: 0, operator: 1, admin: 2 };

/** Whether the signed-in user's role is at least `min`. */
export function useCan(min: Role): boolean {
	const { data } = useMe();
	return !!data && rank[data.role] >= rank[min];
}

function qs(params: Record<string, string | number | undefined | null>): string {
	const p = new URLSearchParams();
	for (const [k, v] of Object.entries(params)) if (v !== undefined && v !== null && v !== '') p.set(k, String(v));
	return p.toString();
}

/**
 * A list endpoint, a page at a time (server-side search and filters). `items` is every
 * loaded row; call `fetchNextPage` (or render <LoadMore>) for more.
 */
export function usePaged<T>(
	key: QueryKey,
	path: string,
	params: Record<string, string | number | undefined | null> = {},
	opts: { limit?: number; refetchInterval?: number; enabled?: boolean } = {},
) {
	const q = useInfiniteQuery({
		queryKey: [...key, params],
		queryFn: ({ pageParam }) => api.get<Page<T>>(`${path}?${qs({ ...params, cursor: pageParam, limit: opts.limit ?? 50 })}`),
		initialPageParam: undefined as string | undefined,
		getNextPageParam: (p) => p.next_cursor ?? undefined,
		refetchInterval: opts.refetchInterval,
		enabled: opts.enabled,
	});
	const items = q.data?.pages.flatMap((p) => p.items) ?? [];
	const total = q.data?.pages[0]?.total;
	return { ...q, items, total };
}

export const useAuthInfo = () => useQuery({ queryKey: keys.auth, queryFn: () => api.get<AuthInfo>('/auth') });
export const useSummary = () =>
	useQuery({ queryKey: keys.summary, queryFn: () => api.get<Summary>('/devices/summary'), refetchInterval: 30_000 });
export const useDevice = (id: string) =>
	useQuery({ queryKey: keys.device(id), queryFn: () => api.get<DeviceDetail>(`/devices/${id}`) });
export const useScript = (id: number) =>
	useQuery({ queryKey: keys.script(id), queryFn: () => api.get<Script>(`/scripts/${id}`), enabled: id > 0 });
export const useJob = (id: number) => useQuery({ queryKey: keys.job(id), queryFn: () => api.get<JobDetail>(`/jobs/${id}`) });

export const useDevices = (f: { q?: string; group?: string; status?: string; os?: string }) =>
	usePaged<Device>(keys.devices, '/devices', f, { limit: 50 });
export const useGroups = (q?: string) => usePaged<Group>(keys.groups, '/groups', { q });
export const useTokens = () => usePaged<EnrollToken>(keys.tokens, '/enroll-tokens', {});
export const useUsers = (q?: string) => usePaged<User & { sso_enabled?: boolean }>(keys.users, '/users', { q });
export const useScripts = (q?: string) => usePaged<Script>(keys.scripts, '/scripts', { q });
export const useJobs = (q?: string, opts: { limit?: number } = {}) =>
	usePaged<Job>(keys.jobs, '/jobs', { q }, { refetchInterval: 10_000, ...opts });
export const useRuns = (id: number, f: { status?: string; q?: string }) => usePaged<Run>(keys.runs(id), `/jobs/${id}/runs`, f);
export const useAudit = (f: { action?: string; q?: string; user?: string; device?: string }) =>
	usePaged<AuditRow>(keys.audit, '/audit', f, { limit: 100 });

/** Whether SSO is on, from the users list (admins only). */
/** Which agent builds the server can hand out (admin only). Polls while any is being fetched. */
export function useAgentDownloads() {
	return useQuery({
		queryKey: keys.agentDownloads,
		queryFn: () => api.get<AgentDownloads>('/agent-downloads'),
		refetchInterval: (q) => (q.state.data?.binaries.some((b) => b.status === 'fetching') ? 3000 : false),
	});
}

export function useSsoEnabled() {
	return useQuery({
		queryKey: [...keys.users, 'sso'],
		queryFn: async () => (await api.get<{ sso_enabled: boolean }>('/users?limit=1')).sso_enabled,
	});
}

/**
 * A mutation that toasts its error, and on success invalidates `invalidate` and toasts
 * `success` (when given).
 */
export function useAction<V, R = unknown>(
	fn: (v: V) => Promise<R>,
	opts: { invalidate?: QueryKey[]; success?: string | ((r: R, v: V) => string); onSuccess?: (r: R, v: V) => void } = {},
) {
	const qc = useQueryClient();
	return useMutation({
		mutationFn: fn,
		onSuccess: (r, v) => {
			for (const k of opts.invalidate ?? []) qc.invalidateQueries({ queryKey: k });
			qc.invalidateQueries({ queryKey: keys.pick });
			if (opts.success) toast.success(typeof opts.success === 'function' ? opts.success(r, v) : opts.success);
			opts.onSuccess?.(r, v);
		},
		onError: (e) => toast.error(e instanceof ApiError ? e.message : String(e)),
	});
}

/** Keeps loaded device lists and pages live from `/api/v1/events`. */
export function useDeviceEvents(enabled: boolean) {
	const qc = useQueryClient();
	useEffect(() => {
		if (!enabled) return;
		const es = new EventSource('/api/v1/events');
		es.onmessage = (m) => {
			const e = JSON.parse(m.data) as DeviceEvent;
			if (e.kind === 'online' || e.kind === 'offline') {
				const patch = (d: Device): Device =>
					d.id !== e.id
						? d
						: e.kind === 'online'
							? { ...d, online: true, online_since: e.since, last_seen: e.since }
							: { ...d, online: false, online_since: null, last_seen: e.last_seen };
				qc.setQueriesData<InfiniteData<Page<Device>>>({ queryKey: keys.devices }, (data) =>
					data ? { ...data, pages: data.pages.map((p) => ({ ...p, items: p.items.map(patch) })) } : data,
				);
			} else if (e.kind === 'removed') {
				qc.setQueriesData<InfiniteData<Page<Device>>>({ queryKey: keys.devices }, (data) =>
					data ? { ...data, pages: data.pages.map((p) => ({ ...p, items: p.items.filter((d) => d.id !== e.id) })) } : data,
				);
			} else {
				qc.invalidateQueries({ queryKey: keys.devices });
			}
			qc.invalidateQueries({ queryKey: keys.summary });
			qc.invalidateQueries({ queryKey: keys.device(e.id) });
		};
		return () => es.close();
	}, [enabled, qc]);
}

export type { Job };
