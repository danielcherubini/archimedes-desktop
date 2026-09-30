import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import { loadAndApplySettings } from "./lib/settings";
import { applyThemeToDocument } from "./lib/theme";
import "./index.css";

// The app is dark by default (no flash — the CSS default is dark); the
// stored settings (theme + font) apply as soon as they arrive, and a
// "system" value subscribes the live `matchMedia` listener for the app's
// lifetime.
applyThemeToDocument("zai-dark");
void loadAndApplySettings();

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
