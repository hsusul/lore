import React from "react";
import ReactDOM from "react-dom/client";

import "@fontsource-variable/inter";
import "@fontsource-variable/jetbrains-mono";

import App from "./App";
import ErrorBoundary from "./components/ErrorBoundary";
import "./styles.css";

const root = document.getElementById("root");
if (root) {
  ReactDOM.createRoot(root).render(
    <React.StrictMode>
      <ErrorBoundary>
        <App />
      </ErrorBoundary>
    </React.StrictMode>,
  );
}
