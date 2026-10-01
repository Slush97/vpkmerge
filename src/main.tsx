import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import "@fontsource-variable/libre-franklin";
import "@fontsource-variable/libre-franklin/wght-italic.css";
import "@fontsource/federo";
import "@fontsource/ibm-plex-mono/400.css";
import "@fontsource/ibm-plex-mono/500.css";
import "./index.css";
import App from "./App";
import { applyTheme, DEFAULT_ACCENT } from "./lib/theme";

applyTheme("system", DEFAULT_ACCENT);

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
