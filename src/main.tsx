import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import { applyThemeToDocument } from "./lib/theme";
import "./index.css";

// The app is dark by default (no switcher).
applyThemeToDocument("zai-dark");

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
