// Shapes of /api/v1, matching server/src/api.rs (and Inventory in proto/src/lib.rs).

export type Role = 'viewer' | 'operator' | 'admin';

export interface Me {
	id: number;
	username: string;
	role: Role;
}

export interface AuthInfo {
	password_login: boolean;
	providers: { id: string; label: string }[];
	setup_required: boolean;
}

export type Attention = 'reboot_pending' | 'firewall_off' | 'low_disk' | 'av_off' | 'offline_7d';

export interface Device {
	id: string;
	name: string;
	hostname: string;
	os: string;
	platform: string;
	group_id: number | null;
	group: string | null;
	online: boolean;
	online_since: number | null;
	last_seen: number;
	last_boot: number | null;
	ip: string | null;
	public_ip: string | null;
	serial: string | null;
	model: string | null;
	logged_in_users: string[];
	attention: Attention[];
}

export interface Inventory {
	collected_at: number;
	hostname: string | null;
	domain: string | null;
	os: { name: string | null; version: string | null; build: string | null; kernel: string | null; arch: string | null };
	uptime_secs: number | null;
	last_boot: number | null;
	logged_in_users: string[];
	hardware: { manufacturer: string | null; model: string | null; serial: string | null };
	cpu: { model: string | null; cores: number | null; threads: number | null };
	memory: { total: number | null; used: number | null };
	disks: { mount: string; fs: string | null; total: number; free: number }[];
	network: { name: string; mac: string | null; ips: string[] }[];
	public_ip: string | null;
	software: { name: string; version: string | null; publisher: string | null; install_date: string | null }[];
	security: {
		encryption: { volume: string; status: string }[] | null;
		antivirus: { name: string; enabled: boolean | null; realtime: boolean | null; signature_age_days: number | null } | null;
		firewall: { profile: string; enabled: boolean | null }[];
		pending_reboot: boolean | null;
	};
}

export type RunStatus = 'queued' | 'running' | 'ok' | 'failed' | 'timeout' | 'offline' | 'expired' | string;

export interface Run {
	id: number;
	device_id: string;
	device_name: string;
	via: string | null;
	status: RunStatus;
	exit_code: number | null;
	stdout: string;
	stderr: string;
	queued: number;
	started: number | null;
	finished: number | null;
}

export interface DeviceRun extends Run {
	job_id: number;
	job_name: string;
}

export interface DeviceDetail {
	device: Device;
	inventory: Inventory | null;
	inventory_at: number | null;
	runs: DeviceRun[];
}

export interface Group {
	id: number;
	name: string;
	devices: number;
}

export interface EnrollToken {
	id: number;
	group: string | null;
	expires: number;
	max_uses: number;
	uses: number;
}

export interface NewEnrollToken {
	token: string;
	server: string;
	linux: string;
	windows: string;
}

export interface Identity {
	provider: string;
	label: string;
	email: string | null;
}

export interface User {
	id: number;
	username: string;
	role: Role;
	created: number;
	email: string | null;
	identities: Identity[];
}

export interface AuditRow {
	id: number;
	ts: number;
	user: string | null;
	device: string | null;
	action: string;
	detail: string;
}

export interface AuditPage {
	rows: AuditRow[];
	next: number | null;
	actions: string[];
}

export type Shell = 'powershell' | 'cmd' | 'bash' | 'sh';

export interface Script {
	id: number;
	name: string;
	shell: Shell;
	timeout_s: number;
	creator: string | null;
	updated: number;
	body?: string;
}

export interface Counts {
	queued: number;
	running: number;
	ok: number;
	failed: number;
	timeout: number;
	offline: number;
}

export type JobKind = 'script' | 'reboot' | 'shutdown' | 'logoff' | 'lock' | 'wake';

export interface Job {
	id: number;
	name: string;
	kind: JobKind;
	task: string;
	script_id: number | null;
	target: string;
	schedule: string | null;
	next_fire: number | null;
	last_fired: number | null;
	enabled: boolean;
	creator: string | null;
	created: number;
	timeout_s: number;
	expires_after: number;
	counts?: Counts;
}

export interface JobDetail {
	job: Job;
	runs: Run[];
}

export interface NewJob {
	name?: string;
	script_id?: number;
	action?: Exclude<JobKind, 'script'>;
	delay_s?: number;
	message?: string;
	devices?: string[];
	group_id?: number;
	all?: boolean;
	cron?: string;
	expires_h?: number;
	confirm?: string;
}

/** `/api/v1/jobs/{id}/events`. */
export type JobEvent =
	| { kind: 'output'; job_id: number; run_id: number; stream: 'stdout' | 'stderr'; data: string; end: number }
	| {
			kind: 'status';
			job_id: number;
			run_id: number;
			status: RunStatus;
			exit_code: number | null;
			started: number | null;
			finished: number | null;
			stdout: string;
			stderr: string;
	  }
	| { kind: 'new_runs'; job_id: number };

/** `/api/v1/events`. */
export type DeviceEvent =
	| { kind: 'online'; id: string; since: number }
	| { kind: 'offline'; id: string; last_seen: number }
	| { kind: 'inventory'; id: string }
	| { kind: 'removed'; id: string };
