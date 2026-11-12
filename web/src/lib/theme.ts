import { useEffect, useState } from 'react';

export type ThemeChoice = 'system' | 'light' | 'dark';

function read(): ThemeChoice {
	try {
		const v = localStorage.getItem('deskhand-theme');
		return v === 'light' || v === 'dark' ? v : 'system';
	} catch {
		return 'system';
	}
}

export function applyTheme(t: ThemeChoice = read()) {
	if (t === 'system') document.documentElement.removeAttribute('data-theme');
	else document.documentElement.setAttribute('data-theme', t);
}

/** Follows the system unless the user picked light or dark (kept in this browser). */
export function useTheme() {
	const [theme, set] = useState<ThemeChoice>(read);
	useEffect(() => applyTheme(theme), [theme]);
	return {
		theme,
		setTheme: (t: ThemeChoice) => {
			try {
				if (t === 'system') localStorage.removeItem('deskhand-theme');
				else localStorage.setItem('deskhand-theme', t);
			} catch {
				/* private mode: just this tab */
			}
			set(t);
		},
	};
}
