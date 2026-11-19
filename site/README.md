# HiyoDyna website

Static Astro site for the HiyoDyna product page. It builds to `dist/` and deploys to GitHub Pages at `https://hiyo41.github.io/HiyoDyna/`.

## Develop

```powershell
npm ci
npm run dev
```

`astro dev` supports `--background`, `astro dev status`, `astro dev logs`, and `astro dev stop`.

## Check

```powershell
npm test
npm run check
$env:PAGES_BASE="/HiyoDyna"; npm run build
node tests/check-build.mjs
```

`PAGES_BASE` must match the Pages path in CI; without it the site builds for `/`.

## Structure

- `src/lib/` holds pure TypeScript: clock formatting, the superellipse path, spring physics, settings, release selection.
- `src/scripts/` wires the island, playground, and download button to the DOM.
- `src/components/` holds the Astro markup.
- `src/styles/global.css` carries the design tokens.
- `public/fonts/OFL.txt` covers the bundled Inter subset.

The fonts are subsets of the app's `assets/fonts` files; regenerate them with `pyftsubset` if the copy needs characters outside Latin, punctuation, `·`, dashes, and `→`.
