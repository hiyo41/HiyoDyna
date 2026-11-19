# HiyoDyna website design system

The site is built around one picture: the HiyoDyna capsule at the top of a dark desktop, alive. Everything else is quiet editorial support. Source values come from the app (`src/animation.rs`, `src/geometry.rs`, `src/render.rs`, `src/settings.rs`, `src/text.rs`).

## Direction

- Near-black desktop, soft ice-blue light behind the capsule, fine grain.
- One authored motion moment: the capsule's spring. No scroll reveals, no entrance animation on every section.
- No cards, no icon tiles, no eyebrow labels above headings, no pill buttons, no gradient text.
- The capsule is the only fully rounded shape on the page.

## Color

| Token | Value | Use |
| --- | --- | --- |
| Ink | `#0A0B0C` | Page background |
| Surface | `#141617` | Raised areas |
| Capsule top | `#191B1C` | Capsule gradient start |
| Capsule bottom | `#111314` | Capsule gradient end |
| Text | `#FAF8F7` | Primary text |
| Secondary | `#B8ACA9` | Body copy |
| Muted | `#A99D9A` | Micro labels |
| Hairline | `#FFFFFF` 8% | Dividers |
| Ice | `#8FC7FF` | Default accent |
| Amber | `#FFC478` | Accent preset |
| Mint | `#96F0C8` | Accent preset |

Body and secondary text pass AA on ink. Accent is used for indicators and small marks, not for long body copy.

## Type

- Body and UI: Inter Medium, 14–16px, line-height 1.6–1.7.
- Display: Inter Display SemiBold, tracking -0.03em, balanced headings, max 5.4rem.
- Measure: body 34rem maximum; FAQ 34rem; hero lede 40rem.
- Micro labels: 11px minimum, 0.14em tracking, uppercase. Never below 11px.
- No third family. Inter is a brand font bundled with the app.

## Shape

- Compact capsule: 184×54, radius 27. Expanded: 420×150, radius 30.
- Exponent-3 superellipse, not a CSS border-radius pill.
- The island is rendered in SVG so the path, rim, shadow, and halo keep the silhouette.
- Rim is white 18% at the top fading to 5% at the bottom. Hover deepens the drop shadow instead of lifting the shape. The halo is the accent at 18% behind the path.

## Motion

- Expand response 0.30s, damping 0.62; collapse 0.32s, damping 0.76.
- Slow 0.38/0.42, fast 0.22/0.24. Bounce low 0.78/0.85, medium 0.62/0.76, high 0.50/0.64.
- Hover grace 160ms. Hover, focus, tap, and Enter/Space expand; Escape collapses and keeps it closed until blur or another explicit open.
- Reduced motion removes the automatic spring, overshoot, and halo. A Motion/Bounce change or the Preview motion button runs one spring preview with the halo, then records the new setting.
- One shared animation loop; pause offscreen and on hidden tabs.

## Layout

- 1120px page width, 20–32px gutter, `clamp()` section rhythm.
- Hero: island at the top, then headline, lede, actions.
- Below: facts strip, features, playground, native, install, FAQ, footer.
- Mobile: single column, 320px minimum, no horizontal scroll.

## Components

- **Island:** SVG capsule + HTML text overlay; two cross-fading visual layers; a transparent `<button>` overlay carries the stable name and `aria-expanded`; the active clock layer is exposed as content; hint below.
- **Buttons:** solid ivory primary, hairline ghost secondary, 10px radius, 50px minimum height.
- **Settings:** native radios/checkboxes, segmented controls, switches, Reset, Preview.
- **FAQ:** native `<details>` with a drawn plus/minus.
- **Download:** static Releases fallback; GitHub API upgrades the label and asset.

## Copy

- Plain and specific. No "revolutionary", "seamless", "unleash", "elevate", "next-level".
- Numbers only when true. Name the platform and the caveats.
- "Dynamic Island-style"; footer states no affiliation with Apple.

## Accessibility

- Keyboard focus on every control: 2px accent outline at a 3px offset for buttons and form controls; the island uses a 2px accent rim on the capsule because its focus box is larger than the shape.
- The island is a real button overlay with a stable accessible name (`HiyoDyna clock preview`), `aria-expanded`, and the hint as its description. The visible clock text sits outside that control's name and only the active layer is exposed.
- `prefers-reduced-motion` respected; touch targets at least 42px.
- Lighthouse accessibility 100 on the shipped page.

## Bans

- Eyebrows or kickers above headings.
- Pill-shaped buttons.
- Card grids and nested cards.
- Icon tiles above headings.
- Gradient text.
- Decorative glass or blur.
- Text below 11px.
