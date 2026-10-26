import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";
import tailwindcss from "tailwindcss";

import tailwindConfig from "./tailwind.config";

// The Rust core is the heavy lifter; the UI is a thin Tauri front end.
export default defineConfig({
  plugins: [react()],
  css: { postcss: { plugins: [tailwindcss(tailwindConfig)] } },
  // Tauri expects a fixed dev port and does not clear the screen on error.
  clearScreen: false,
  server: { port: 5173, strictPort: true },
  build: { outDir: "dist", target: "es2021" },
  test: {
    environment: "jsdom",
    globals: true,
    include: ["src/**/*.test.{ts,tsx}"],
    setupFiles: ["src/test-setup.ts"],
    // tokens.test.ts reads tokens.css as text; other stylesheets stay stubbed.
    css: { include: [/tokens\.css/] },
  },
});
