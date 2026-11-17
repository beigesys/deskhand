import * as React from 'react';
import { Search } from 'lucide-react';
import { Input } from './form';

/** `v` once it has stopped changing for `ms`, trimmed. */
export function useDebounce(v: string, ms = 200) {
	const [d, setD] = React.useState(v);
	React.useEffect(() => {
		const t = setTimeout(() => setD(v.trim()), ms);
		return () => clearTimeout(t);
	}, [v, ms]);
	return d;
}

export function SearchBox({ value, onChange, placeholder }: { value: string; onChange: (v: string) => void; placeholder: string }) {
	return (
		<div className="relative mb-3 max-w-sm">
			<Search className="pointer-events-none absolute left-2.5 top-1/2 size-4 -translate-y-1/2 text-subtle" />
			<Input type="search" value={value} onChange={(e) => onChange(e.target.value)} placeholder={placeholder} className="pl-8" aria-label={placeholder} />
		</div>
	);
}
