import * as React from 'react';
import { Loader2 } from 'lucide-react';
import { Button } from './button';

/**
 * The bottom of a paged list: loads the next page when it scrolls into view, with a
 * button as the fallback.
 */
export function LoadMore({
	hasNextPage,
	isFetchingNextPage,
	fetchNextPage,
	shown,
	total,
}: {
	hasNextPage: boolean;
	isFetchingNextPage: boolean;
	fetchNextPage: () => unknown;
	shown?: number;
	total?: number;
}) {
	const ref = React.useRef<HTMLDivElement>(null);
	React.useEffect(() => {
		const el = ref.current;
		if (!el || !hasNextPage) return;
		const io = new IntersectionObserver((e) => {
			if (e[0]?.isIntersecting && !isFetchingNextPage) fetchNextPage();
		});
		io.observe(el);
		return () => io.disconnect();
	}, [hasNextPage, isFetchingNextPage, fetchNextPage]);
	if (!hasNextPage) {
		return total != null && shown != null && total > 0 ? (
			<div className="px-3 py-2 text-center text-xs text-muted">
				{shown} of {total}
			</div>
		) : null;
	}
	return (
		<div ref={ref} className="flex items-center justify-center gap-2 px-3 py-2 text-xs text-muted">
			{isFetchingNextPage ? (
				<>
					<Loader2 className="size-3.5 animate-spin" /> Loading more…
				</>
			) : (
				<Button size="sm" variant="ghost" onClick={() => fetchNextPage()}>
					Load more{total != null && shown != null ? ` (${shown} of ${total})` : ''}
				</Button>
			)}
		</div>
	);
}
