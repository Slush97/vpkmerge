import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import "@fontsource-variable/inter/opsz.css";
import "@fontsource-variable/geist-mono";
import "./index.css";
import App from "./App";
import { applyTheme, DEFAULT_ACCENT } from "./lib/theme";

applyTheme("system", DEFAULT_ACCENT);

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
