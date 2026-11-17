import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { RouterProvider } from '@tanstack/react-router';
import { Toaster } from 'sonner';
import { ApiError } from '@/api/client';
import { TooltipProvider } from '@/components/ui/overlay';
import { applyPalette, applyTheme } from '@/lib/theme';
import { router } from '@/router';
import './index.css';

applyTheme();
applyPalette();

const queryClient = new QueryClient({
	defaultOptions: {
		queries: {
			staleTime: 5_000,
			retry: (n, e) => !(e instanceof ApiError && e.status < 500) && n < 2,
			refetchOnWindowFocus: true,
		},
	},
});

createRoot(document.getElementById('root')!).render(
	<StrictMode>
		<QueryClientProvider client={queryClient}>
			<TooltipProvider>
				<RouterProvider router={router} />
				<Toaster
					position="bottom-right"
					toastOptions={{
						className: '!rounded !border !border-line-strong !bg-surface !text-fg !font-sans !text-[13px] !shadow-none',
					}}
				/>
			</TooltipProvider>
		</QueryClientProvider>
	</StrictMode>,
);
