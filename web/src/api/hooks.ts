import { useEffect } from 'react';
import { useMutation, useQuery, useQueryClient, type QueryKey } from '@tanstack/react-query';
import { toast } from 'sonner';
import { api, ApiError } from './client';
import type {
	AuditPage,
	AuthInfo,
	Device,
	DeviceDetail,
	DeviceEvent,
	EnrollToken,
	Group,
	Job,
	JobDetail,
	Me,
	Role,
	Script,
	User,
} from './types';

export const keys = {
	me: ['me'] as const,
	auth: ['auth'] as const,
	devices: ['devices'] as const,
	device: (id: string) => ['device', id] as const,
	groups: ['groups'] as const,
	tokens: ['enroll-tokens'] as const,
	users: ['users'] as const,
	audit: (f: object) => ['audit', f] as const,
	scripts: ['scripts'] as const,
	script: (id: number) => ['script', id] as const,
	jobs: ['jobs'] as const,
	job: (id: number) => ['job', id] as const,
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

export const useAuthInfo = () => useQuery({ queryKey: keys.auth, queryFn: () => api.get<AuthInfo>('/auth') });
export const useDevices = () => useQuery({ queryKey: keys.devices, queryFn: () => api.get<Device[]>('/devices') });
export const useDevice = (id: string) =>
	useQuery({ queryKey: keys.device(id), queryFn: () => api.get<DeviceDetail>(`/devices/${id}`) });
export const useGroups = () => useQuery({ queryKey: keys.groups, queryFn: () => api.get<Group[]>('/groups') });
export const useTokens = () => useQuery({ queryKey: keys.tokens, queryFn: () => api.get<EnrollToken[]>('/enroll-tokens') });
export const useUsers = () =>
	useQuery({ queryKey: keys.users, queryFn: () => api.get<{ users: User[]; sso_enabled: boolean }>('/users') });
export const useScripts = () => useQuery({ queryKey: keys.scripts, queryFn: () => api.get<Script[]>('/scripts') });
export const useScript = (id: number) =>
	useQuery({ queryKey: keys.script(id), queryFn: () => api.get<Script>(`/scripts/${id}`), enabled: id > 0 });
export const useJobs = () =>
	useQuery({ queryKey: keys.jobs, queryFn: () => api.get<Job[]>('/jobs'), refetchInterval: 10_000 });
export const useJob = (id: number) => useQuery({ queryKey: keys.job(id), queryFn: () => api.get<JobDetail>(`/jobs/${id}`) });

export function useAudit(f: { action: string; q: string }) {
	return useQuery({
		queryKey: keys.audit(f),
		queryFn: () => api.get<AuditPage>(`/audit?${new URLSearchParams({ ...f, limit: '100' })}`),
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
			if (opts.success) toast.success(typeof opts.success === 'function' ? opts.success(r, v) : opts.success);
			opts.onSuccess?.(r, v);
		},
		onError: (e) => toast.error(e instanceof ApiError ? e.message : String(e)),
	});
}

/** Keeps device lists and pages live from `/api/v1/events`. */
export function useDeviceEvents(enabled: boolean) {
	const qc = useQueryClient();
	useEffect(() => {
		if (!enabled) return;
		const es = new EventSource('/api/v1/events');
		es.onmessage = (m) => {
			const e = JSON.parse(m.data) as DeviceEvent;
			if (e.kind === 'online' || e.kind === 'offline') {
				qc.setQueryData<Device[]>(keys.devices, (list) =>
					list?.map((d) =>
						d.id !== e.id
							? d
							: e.kind === 'online'
								? { ...d, online: true, online_since: e.since, last_seen: e.since }
								: { ...d, online: false, online_since: null, last_seen: e.last_seen },
					),
				);
				// A device we haven't seen yet (just enrolled): fetch the list.
				const known = qc.getQueryData<Device[]>(keys.devices)?.some((d) => d.id === e.id);
				if (!known) qc.invalidateQueries({ queryKey: keys.devices });
			} else if (e.kind === 'removed') {
				qc.setQueryData<Device[]>(keys.devices, (list) => list?.filter((d) => d.id !== e.id));
			} else {
				qc.invalidateQueries({ queryKey: keys.devices });
			}
			qc.invalidateQueries({ queryKey: keys.device(e.id) });
		};
		return () => es.close();
	}, [enabled, qc]);
}
