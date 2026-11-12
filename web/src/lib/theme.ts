import { useEffect, useState, useSyncExternalStore } from 'react';

export type ThemeChoice = 'system' | 'light' | 'dark';
export type Palette = 'graphite' | 'harbor' | 'field' | 'paper' | 'navy';

/** The palettes theme.css defines, with the swatch the picker shows. */
export const PALETTES: { id: Palette; label: string; swatch: string }[] = [
	{ id: 'graphite', label: 'Graphite + orange', swatch: '#17191c' },
	{ id: 'harbor', label: 'Harbor teal + brass', swatch: '#0f2a2a' },
	{ id: 'field', label: 'Field olive + orange', swatch: '#262a1e' },
	{ id: 'paper', label: 'Light sidebar + orange', swatch: '#ffffff' },
	{ id: 'navy', label: 'Navy (original)', swatch: '#0b1626' },
];

function get(key: string): string | null {
	try {
		return localStorage.getItem(key);
	} catch {
		return null;
	}
}

function put(key: string, v: string | null) {
	try {
		if (v === null) localStorage.removeItem(key);
		else localStorage.setItem(key, v);
	} catch {
		/* private mode: this tab only */
	}
}

function readTheme(): ThemeChoice {
	const v = get('deskhand-theme');
	return v === 'light' || v === 'dark' ? v : 'system';
}

const isPalette = (v: string | null): v is Palette => PALETTES.some((p) => p.id === v);

/** A user's palette; before anyone signs in, the last one used in this browser. */
function readPalette(user?: string): Palette {
	const v = (user && get(`deskhand-palette:${user}`)) || get('deskhand-palette');
	return isPalette(v) ? v : 'graphite';
}

export function applyTheme(t: ThemeChoice = readTheme()) {
	if (t === 'system') document.documentElement.removeAttribute('data-theme');
	else document.documentElement.setAttribute('data-theme', t);
}

export function applyPalette(p: Palette = readPalette()) {
	document.documentElement.setAttribute('data-pal', p);
}

/** Follows the system unless the user picked light or dark (kept in this browser). */
export function useTheme() {
	const [theme, set] = useState<ThemeChoice>(readTheme);
	useEffect(() => applyTheme(theme), [theme]);
	return {
		theme,
		setTheme: (t: ThemeChoice) => {
			put('deskhand-theme', t === 'system' ? null : t);
			set(t);
		},
	};
}

// One palette for the whole page, shared by every component that shows or changes it.
let current: Palette = readPalette();
const listeners = new Set<() => void>();
function setCurrent(p: Palette) {
	current = p;
	applyPalette(p);
	listeners.forEach((l) => l());
}

function subscribe(l: () => void) {
	listeners.add(l);
	return () => {
		listeners.delete(l);
	};
}

/** The signed-in user's palette, kept per user in this browser. */
export function usePalette(user: string | undefined) {
	const palette = useSyncExternalStore(subscribe, () => current);
	// Signing in switches to that user's own palette.
	useEffect(() => {
		if (user) setCurrent(readPalette(user));
	}, [user]);
	return {
		palette,
		setPalette: (p: Palette) => {
			if (user) put(`deskhand-palette:${user}`, p);
			put('deskhand-palette', p);
			setCurrent(p);
		},
	};
}
