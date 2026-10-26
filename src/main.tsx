import React from "react";
import ReactDOM from "react-dom/client";

import App from "./App";
import ErrorBoundary from "./components/ErrorBoundary";
import "./styles.css";

/**
 * `npm run dev` in a plain browser has no Tauri shell, so serve the archive UI
 * from in-memory sample data. The branch needs both checks: `import.meta.env.DEV`
 * is false in a production build, so the bundler drops the mock import entirely,
 * and `__TAURI_INTERNALS__` is present inside the shell, which keeps
 * `npm run tauri dev` on the real backend.
 */
async function installBrowserMock(): Promise<void> {
  if (import.meta.env.DEV && !("__TAURI_INTERNALS__" in window)) {
    const { installIpcMock } = await import("./mock/ipc-mock");
    installIpcMock();
  }
}

const root = document.getElementById("root");
if (root) {
  void installBrowserMock().then(() => {
    ReactDOM.createRoot(root).render(
      <React.StrictMode>
        <ErrorBoundary>
          <App />
        </ErrorBoundary>
      </React.StrictMode>,
    );
  });
}
