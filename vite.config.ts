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
    // Tests read tokens.css, styles.css and the bundled font faces as text; nothing else needs real CSS.
    css: { include: [/(tokens|styles)\.css/, /@fontsource\//] },
  },
});
