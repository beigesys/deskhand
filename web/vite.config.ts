import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
import tailwindcss from '@tailwindcss/vite';
import path from 'node:path';

// `bun run dev` serves the app on 5173 and sends everything the Rust server owns to it.
const server = process.env.DESKHAND_DEV_SERVER ?? 'http://127.0.0.1:8080';
const toServer = { target: server, changeOrigin: false, ws: true };

export default defineConfig({
	plugins: [react(), tailwindcss()],
	resolve: { alias: { '@': path.resolve(import.meta.dirname, 'src') } },
	server: {
		proxy: {
			'/api': toServer,
			'/auth': toServer,
			'/d/': toServer,
			'/classic': toServer,
			'/agent': toServer,
		},
	},
	build: { outDir: 'dist', emptyOutDir: true, chunkSizeWarningLimit: 1200 },
});
