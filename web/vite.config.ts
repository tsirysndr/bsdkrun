import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// A plain web app: no fixed port and no Tauri dev-host dance. The daemon's
// GraphQL endpoint is configured at runtime from the UI, not baked in here, so
// there is nothing to proxy and this build works from any static host.
export default defineConfig({
  plugins: [react()],
  build: {
    target: "es2021",
    // No sourcemaps: this bundle is embedded into the bsdkrun binary, where
    // 4.8 MB of maps would be pure weight.
    sourcemap: false,
  },
});
