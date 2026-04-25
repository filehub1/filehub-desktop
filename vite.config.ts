import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
import path from 'path';

// Reuse filehub-server's renderer source directly
const rendererRoot = path.resolve(__dirname, '../filehub-server/src/renderer');

export default defineConfig({
  plugins: [react()],
  root: rendererRoot,
  base: './',
  build: {
    outDir: path.resolve(__dirname, 'src-tauri/dist'),
    emptyOutDir: true,
  },
  resolve: {
    alias: {
      '@': rendererRoot,
    },
  },
});
