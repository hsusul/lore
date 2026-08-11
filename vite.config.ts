import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";

// The Rust core is the heavy lifter; the UI is a thin Tauri front end.
export default defineConfig({
  plugins: [react()],
  // Tauri expects a fixed dev port and does not clear the screen on error.
  clearScreen: false,
  server: { port: 5173, strictPort: true },
  build: { outDir: "dist", target: "es2021" },
  test: {
    environment: "jsdom",
    globals: true,
    include: ["src/**/*.test.{ts,tsx}"],
  },
});
