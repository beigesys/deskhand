import { clsx, type ClassValue } from 'clsx';
import { twMerge } from 'tailwind-merge';

export function cn(...inputs: ClassValue[]) {
	return twMerge(clsx(inputs));
}

const now = () => Math.floor(Date.now() / 1000);

/** "4m", "3h", "12d" for a span in seconds. */
export function span(s: number): string {
	s = Math.max(0, Math.round(s));
	if (s < 60) return `${s}s`;
	if (s < 3600) return `${Math.floor(s / 60)}m`;
	if (s < 86400) return `${Math.floor(s / 3600)}h`;
	return `${Math.floor(s / 86400)}d`;
}

export const ago = (t: number | null | undefined) => (t ? `${span(now() - t)} ago` : '—');
export const until = (t: number | null | undefined) => (t ? `in ${span(t - now())}` : '—');

export function datetime(t: number | null | undefined): string {
	if (!t) return '—';
	return new Date(t * 1000).toLocaleString(undefined, {
		year: 'numeric',
		month: 'short',
		day: 'numeric',
		hour: '2-digit',
		minute: '2-digit',
	});
}

export function bytes(n: number | null | undefined): string {
	if (n == null) return '—';
	const u = ['B', 'KiB', 'MiB', 'GiB', 'TiB'];
	let i = 0;
	let v = n;
	while (v >= 1024 && i < u.length - 1) {
		v /= 1024;
		i++;
	}
	return `${v.toFixed(v >= 100 || i === 0 ? 0 : 1)} ${u[i]}`;
}

export function duration(started: number | null, finished: number | null): string {
	if (!started) return '';
	return finished ? span(finished - started) : `${span(now() - started)}…`;
}

export const attentionLabel: Record<string, string> = {
	reboot_pending: 'Reboot pending',
	firewall_off: 'Firewall off',
	low_disk: 'Low disk',
	av_off: 'Antivirus off',
	offline_7d: 'Offline 7d+',
};
