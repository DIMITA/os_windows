# WilOS Design System — "Aurora"

The WilOS visual identity is **glassmorphic acrylic**: translucent
surfaces, soft depth, generous rounding, vivid accent colour. It is
the evolution of what Mica/Acrylic do in Windows 11, pushed further on
modern GPUs. This document is the source of truth for every UI surface
that ships in phase 2 and beyond.

## Brand

- **Name**: WilOS
- **Wordmark**: lowercase `wilos`, geometric sans, slightly extended
- **Mascot**: a stylised aurora glyph (default desktop accent)
- **Tagline**: *"Light, made personal."*

## Colour tokens

Tokens are defined in OKLCH so they preserve perceptual contrast in
both light and dark modes.

| Token              | Light                     | Dark                      |
|--------------------|---------------------------|---------------------------|
| `surface/base`     | `oklch(98% 0.01 250)`     | `oklch(14% 0.02 260)`     |
| `surface/raised`   | `oklch(96% 0.02 250 / .72)` | `oklch(20% 0.03 260 / .60)` |
| `surface/glass`    | `oklch(100% 0 0 / .55)`   | `oklch(22% 0.04 260 / .50)` |
| `text/primary`     | `oklch(18% 0.02 260)`     | `oklch(96% 0.01 250)`     |
| `text/muted`       | `oklch(45% 0.02 260)`     | `oklch(72% 0.02 260)`     |
| `accent/aurora`    | `oklch(70% 0.18 250)`     | `oklch(78% 0.18 250)`     |
| `accent/sunset`    | `oklch(72% 0.18 30)`      | `oklch(78% 0.18 30)`      |
| `border/hairline`  | `oklch(0% 0 0 / .08)`     | `oklch(100% 0 0 / .10)`   |

## Materials

Three layered materials, all rendered by the compositor with GPU
shaders so they stay live (real-time blur, not pre-baked):

1. **Glass** — frosted, ~32 px gaussian blur, 55 % luminosity, 1 px
   inner highlight on top edge.
2. **Aurora** — directional gradient mesh (accent + analogous) at
   30 % opacity behind glass for hero surfaces (lock screen, start).
3. **Mica** — opaque tint sampled from desktop wallpaper, used for
   chrome on power-saving devices where live blur is disabled.

## Geometry

- Corner radius: `8` (controls), `16` (cards), `24` (windows),
  `32` (hero surfaces).
- Default elevation: 0/1/2 only — depth comes from blur and parallax,
  not heavy shadows.
- Grid: 4 px base, 8 px primary.

## Motion

- Springs over easing curves. Default spring: `mass: 1, stiffness: 220,
  damping: 26`.
- Window summon: 240 ms scale + opacity, slight Y-axis parallax.
- Focus shift: never longer than 120 ms.
- Reduce-motion mode disables blur animations and reverts to opacity.

## Typography

- **UI**: Inter Variable.
- **Document**: Source Serif 4.
- **Mono**: JetBrains Mono.
- Type scale: 12 / 13 / 14 / 16 / 20 / 28 / 40 (display).

## Iconography

- Two-tone outline, 1.5 px stroke, 24 × 24 grid.
- Filled variant for selected/active state only.

## Surfaces (macOS-leaning layout)

- **Lock screen**: aurora material, large time and date centered,
  passwordless authentication chip.
- **Top menu bar**: floating glass slab anchored top, 12 px from the
  edges, 14 px rounded. Workspaces as dots (left), live clock
  (centered), status icons (right), tucked-in app menu icon at the
  far left.
- **Bottom dock**: floating glass slab anchored bottom, centered,
  22 px rounded. Pinned launchers + open-app icons. Hover-lift
  animation (8 px translateY, 200 ms aurora spring).
- **Spotlight launcher**: glass card centered on the screen,
  16 px focus ring in `accent/aurora`, fuzzy search, instant results.
- **Notifications**: glass cards top-right, stack vertically, slide
  from the right with the standard spring.
- **Windows**: 16 px rounded chrome, default opacity 0.92 active /
  0.84 inactive so blur shows through. Window controls on the LEFT
  (close / minimise / fullscreen, traffic-light style).
- **Workspace overview**: 3-finger swipe up reveals all workspaces
  as scaled-down glass tiles.

## Mapping to the implementation

The Aurora identity is realised by three configuration surfaces:

| Token area     | Lives in                                                  |
|----------------|-----------------------------------------------------------|
| Window glass   | `hyprland.conf` `decoration { blur, rounding, shadow }`   |
| Bar/dock glass | `waybar/style.css`, `waybar/dock.css`                     |
| Launcher glass | `wofi/style.css`                                          |
| Terminal       | `kitty/kitty.conf` colour palette                         |
| Notifications  | `mako/config` colours + radius                            |

The Hyprland `general.col.active_border` gradient
(`#7CC8FF → #B198FF`) IS the Aurora accent and must match the
gradient used in dock hover and launcher focus.

A full Figma library will live in the (future) `design/` directory and
will be wired to the compositor through generated tokens.
