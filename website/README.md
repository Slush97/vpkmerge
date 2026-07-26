# VPKMERGE website concept

A standalone, responsive landing-page prototype for VPKMERGE. It uses no build step.

## Preview locally

From the repository root:

```sh
python3 -m http.server 4173
```

Then open `http://localhost:4173/website/`.

## Structure

- `index.html` — semantic page content and product narrative
- `styles.css` — period-inspired design system and responsive layout
- `script.js` — mobile navigation, reveal motion, command copying, and footer year
- `assets/` — project-owned image assets used by the page

The page is intentionally framework-free so it can be hosted as-is or moved into a Vite,
Astro, or other static-site project later.
