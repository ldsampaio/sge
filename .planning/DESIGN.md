# Design System Inspired by Codemaster

## 1. Visual Theme & Atmosphere

Codemaster's experience reads like a coding bootcamp at night — a dark studio lit by a single spark of amber light. The canvas defaults to **Offline Black** (`#0F0F0F`), not a neutral dark-mode gray but a warm near-black that feels closer to obsidian than to a code editor's default theme. Against that black, one color is allowed to command attention: **Spark Yellow** (`#FBBA00`). Unlike systems that ration their accent color, Codemaster spends it freely — headlines, icon glyphs, borders, buttons, and badges are all rendered in Spark, because the brand's energy comes from contrast, not restraint.

The signature shape of the identity is angular, not soft: the logo mark is built from interlocking **chevrons and cut diamonds** — sharp V-forms that overlap to create a faceted, almost circuit-board rhythm. This "Spark Weave" motif is the visual fingerprint of the brand, but it is deliberately kept out of interactive UI. Buttons, cards, and photo frames go the opposite direction — **fully rounded pills and soft-radius rectangles** — so the page reads as approachable and easy to navigate even while the brand mark underneath stays sharp and technical. That tension — a faceted, engineered mark sitting on top of soft, friendly interface shapes — is the core visual idea: *precision meets mentorship*.

A second gesture is the **diamond photo mask**. Where other systems crop photography into circles or rounded rectangles, Codemaster crops hero and promotional imagery along the jagged silhouette of its own chevron mark, letting the black canvas bleed into the photo edge. The weave pattern also appears as a **decorative texture strip** — most prominently as a thin footer border and as a low-opacity fragment bleeding off the corner of hero panels — never as a background behind body copy, where it would compete with reading.

Typography is set entirely in **Readex Pro**, an open, rounded geometric sans that softens the mark's sharp angles. Headlines run in **SemiBold**, giving titles enough weight to anchor a dark page without shouting. Body copy, lists, and long-form text run in **Light**, keeping paragraphs airy and legible against black. The system rests on just two weights — a simple, disciplined hierarchy of "loud" and "quiet" text.

**Key Characteristics:**
- Dark **Offline Black** (`#0F0F0F`) canvas by default — Codemaster is a dark-theme-first brand, not a light theme with a dark mode bolted on
- **Spark Yellow** used generously as the primary accent — headlines, borders, icons, and CTAs, not reserved for a single micro-use case
- Sharp **chevron/diamond "Spark Weave"** motif reserved for decoration and photo masking; never used as an interactive UI shape
- Interactive components (buttons, badges, portrait frames) are fully rounded pills/circles — the opposite geometry of the brand mark, by design
- **Diamond-masked photography** instead of rectangular or circular crops for hero and promotional imagery
- Thin **1.5–2px Spark borders** are the primary depth cue on dark surfaces — shadows read poorly on near-black backgrounds
- **Suntech Gradient** (`#FBBA00 → #DB8800`) reserved for hero-scale promotional blocks — a "spotlight" moment, used sparingly
- Circular professor/portrait photography with a thin Spark ring, paired with a small pill-shaped credential badge
- Footer anchored by a dual-brand logo lockup and a thin strip of the Spark Weave pattern along the very bottom edge

## 2. Color Palette & Roles

### Brand Colors (from identity)
- **Spark** (`#FBBA00`): The signature brand color. Used for headlines, primary CTAs, icon glyphs, card borders, badges, and any element that needs to command attention.
- **Offline** (`#0F0F0F`): The default page canvas. A warm near-black rather than a pure digital black, used across hero sections, feature grids, and the footer.
- **Online** (`#F9F9F9`): The light surface. Used for the navigation bar, and for any section that intentionally flips to a light "daytime" register for contrast against the mostly-dark page.
- **Suntech Gradient** (`#FBBA00 → #DB8800`): A warm yellow-to-amber gradient reserved for hero-level promotional blocks (enrollment banners, feature spotlights). Never used on small UI elements — its scale is meant to feel like a moment, not a default fill.

### Supporting Neutrals (derived to complete the system)
- **Deep Offline** (`#050505`): A step darker than the canvas — used for the footer and the very bottom of long pages, giving a sense of "settling" into darkness.
- **Raised Offline** (`#171717`): A step lighter than the canvas — used for card fills and panels that need to sit "above" the base black without a hard edge.
- **Ember Orange** (`#DB8800`): The deep end of the Suntech gradient, used standalone for hover/active states on Spark elements and for secondary accent icons.
- **Bright Spark** (`#FFD466`): A lighter tint of Spark, used for hover states on links/buttons and for the ghost-watermark headline treatment, where full-strength Spark would be too loud.
- **Fog Gray** (`#B3B3B3`): Muted secondary text on dark surfaces — captions, fine print, disabled labels.
- **Graphite Line** (`#2A2A2A`): A low-emphasis divider/border used where a full Spark border would be too strong (e.g., inactive list separators).
- **Online White** (`#F9F9F9`) doubles as primary body text color on dark surfaces, avoiding pure `#FFFFFF`, which reads slightly too cold against Offline Black.

### Semantic
- **Primary text on dark**: Online (`#F9F9F9`)
- **Primary text on light**: Offline (`#0F0F0F`)
- **Ghost watermark text**: Deep Offline-tinted gray, roughly `#1A1A1A` on the `#0F0F0F` canvas — a near-invisible tonal step, mirroring a "dark-on-dark" watermark instead of light-on-light
- **Link / interactive text**: Spark (`#FBBA00`), hover state Bright Spark (`#FFD466`)
- **Disabled / muted**: Fog Gray (`#B3B3B3`) at reduced opacity

### Gradient System
The Suntech Gradient is the only programmatic gradient in the system. It is applied at two scales only:
- **Promotional banners**: full-bleed rounded rectangles announcing enrollment/deadlines, gradient fill left-to-right or diagonal
- **Glow shadows**: soft Spark-tinted halos (`rgba(251,186,0, 0.15–0.35)`) beneath primary CTAs and the pricing card, simulating light emanating from the brand color rather than a literal gradient fill

## 3. Typography Rules

### Font Family
- **Primary — Headlines & Emphasis**: `Readex Pro SemiBold` — used for all headlines, section titles, card titles, eyebrow labels, button labels, and navigation.
- **Primary — Body & Long-form**: `Readex Pro Light` — used for paragraphs, lists, captions, fine print, and any extended reading content.
- **Fallback stack**: `"Readex Pro", "Arial", sans-serif` — Readex Pro is an open Google Font, so no proprietary substitute is required; it should load reliably across all platforms.

### Hierarchy

| Role | Size | Weight | Line Height | Letter Spacing | Notes |
|------|------|--------|-------------|-----------------|-------|
| H1 (hero) | 56px | SemiBold | 1.05 | -1% | Hero headlines on the dark canvas, 1–3 lines |
| H2 (section) | 32px | SemiBold | 1.15 | -0.5% | Section titles such as "Nossos professores" or "Perguntas frequentes" |
| H3 (card title) | 20px | SemiBold | 1.25 | 0% | Curriculum card titles, pricing card headline, feature titles |
| Eyebrow | 12px | SemiBold | 1 | +6% | Uppercase, paired with a small Spark diamond bullet instead of a dot |
| Body paragraph | 16px | Light | 1.6 | 0% | Default reading copy on dark or light surfaces |
| Button / Nav label | 15px | SemiBold | 1 | -1% | Compact, no text-transform |
| Footer link | 14px | Light | 1.5 | 0% | Lighter weight for airy density in the dark footer |
| Footer column header | 12px | SemiBold | 1 | +6% | Uppercase, Fog Gray, short tracking |

### Principles
- **Two weights, two jobs**: SemiBold always means "pay attention here" (headlines, buttons, badges, eyebrows). Light always means "read comfortably here" (paragraphs, lists, fine print). Never invert this pairing.
- **Spark is a typographic color, not just a UI color**: headline text itself is frequently set directly in Spark Yellow on the Offline canvas, rather than staying black/white with yellow only in accents.
- **Uppercase is reserved for the eyebrow and footer-column scale**. Section titles and body copy are always sentence case.
- **Generous line-height on body copy (1.6)** compensates for the Light weight's reduced stroke contrast against a dark background, keeping paragraphs legible at typical reading sizes.
- **One typeface, two registers**. Resist introducing a second family for contrast — hierarchy comes from the SemiBold/Light split and from scale, not from mixing fonts.

## 4. Component Stylings

### Buttons

**Primary — Spark Pill**
- Background: Spark (`#FBBA00`)
- Text: Offline Black (`#0F0F0F`) — dark text on yellow keeps contrast high and legible
- Border: none
- Radius: 999px (full pill)
- Padding: 12px 28px
- Font: Readex Pro SemiBold 15px
- Use for: primary marketing and conversion CTAs ("Faça sua inscrição")

**Secondary — Outline Pill**
- Background: transparent
- Text: Spark (`#FBBA00`)
- Border: 1.5px solid Spark
- Radius: 999px
- Padding: 12px 28px
- Font: Readex Pro SemiBold 15px
- Use for: supporting actions paired with a primary CTA ("Baixe as ementas completas")

**Gradient — Suntech Pill**
- Background: Suntech Gradient (`#FBBA00 → #DB8800`)
- Text: Online White (`#F9F9F9`)
- Border: none
- Radius: 999px
- Padding: 12px 32px
- Use for: CTAs living inside gradient promotional banners, where a solid Spark button would disappear into the gradient background

**Ghost / Text Link**
- Background: none
- Text: Spark, underline appears on hover
- Font: Readex Pro SemiBold 15px
- Use for: nav links, FAQ toggles, inline text actions

### Cards & Containers

**Hero Media Frame (Diamond Mask)**
- Photography cropped along the chevron/diamond silhouette of the brand mark rather than a rectangle
- Sits directly on the Offline canvas, bleeding into black at its jagged edge
- No shadow — the mask itself provides the visual interest

**Feature Item**
- No background tile; a small Spark line-icon glyph (~32–40px) sits directly on the canvas above an H3 title (SemiBold) and a short Light body line below
- Arranged in a grid of 2–3 columns on desktop, collapsing to a single column on mobile

**Curriculum / Accordion Card**
- Background: Raised Offline (`#171717`)
- Border: 1.5px solid Spark
- Radius: 20px
- Padding: 24–32px
- Title in SemiBold Spark, body copy in Light Online White
- Stacked vertically with 12–16px gaps between rows

**Professor Portrait**
- Shape: perfect circle, diameter ~140–160px
- Border: 2px solid Spark ring around the photo
- Name below in SemiBold Spark
- Small pill-shaped credential badge beneath the name: transparent fill, 1px Spark border, Spark text, radius 999px, padding 4px 14px

**Pricing Card**
- Background: Raised Offline (`#171717`)
- Border: 2px solid Spark
- Radius: 28px
- Contains: logo lockup, struck-through original price in Fog Gray, large current price in Spark, a Spark or Suntech Gradient CTA pill, and fine print in Fog Gray
- Shadow: soft Spark glow (`rgba(251,186,0,0.25) 0px 24px 48px`)

**Promotional Banner (Suntech Block)**
- Background: Suntech Gradient, diagonal or left-to-right
- Radius: 24–40px
- Contains: white/negative logo lockup, headline in Online White, a diamond-masked photo on the opposite side
- Use sparingly — no more than one per page, reserved for deadlines/enrollment announcements

**FAQ Row**
- Background: Raised Offline or transparent on canvas
- Border: 1.5px solid Spark, radius 16px
- Collapsed state: question in SemiBold Online White with a Spark plus/chevron icon at the right
- Rows stacked with 8–12px gaps

### Inputs & Forms
Minimal form surface on the marketing page:
- Text inputs: 1px Graphite Line border at rest, transitioning to 1.5px Spark border on focus, radius 999px, Raised Offline background, Online White text
- Search/filter toggles follow the same pill treatment as buttons

### Navigation
- A full-width **Online White** bar sits flush at the top of the viewport (not a floating inset pill) — the one place the light surface color dominates
- Contains the dual-brand lockup (Codemaster + partner logo) on the left, primary links center-right, all set in Offline Black
- The active/current page link is highlighted in Spark Yellow
- **Mobile**: collapses to logo lockup + hamburger menu; menu opens into a full-screen dark overlay with links stacked and set in Spark/Online White

### Image Treatment
- **Diamond masking** for hero and promotional photography — the signature move, replacing rectangular or circular crops
- **Circular masking** reserved specifically for people (professor/team portraits), always paired with the Spark ring
- Standard rectangular crops are avoided wherever the brand mark's silhouette can be applied instead

### Decorative Spark Weave Pattern
- Thin interlocking chevron/diamond lines in Spark Yellow on Offline Black, used as: a full-width decorative strip along the very bottom of the footer; a low-opacity fragment bleeding off the corner of a hero panel; or a subtle textured backdrop behind an isolated section divider
- Never placed behind body copy or inside cards — it is a framing device, not a background fill for readable content

### Footer
- Background: Deep Offline (`#050505`)
- Text: Online White, Fog Gray for secondary rows
- Centered dual-brand logo lockup (Codemaster + partner institution), separated by a thin vertical divider
- Minimal link structure — legal/contact rows in Light 14px
- A thin **Spark Weave** pattern strip runs along the very bottom edge as the closing visual signature

## 5. Layout Principles

### Spacing System
- **Base unit**: 8px
- **Scale**: 8 / 16 / 24 / 32 / 48 / 64 / 96 / 128
- **Section vertical padding**: ~96–128px desktop, ~48–64px mobile
- **Card internal padding**: 24–32px desktop, ~16–24px mobile

### Grid & Container
- **Max content width**: ~1200–1280px, centered, with 48–96px gutters
- **Hero**: asymmetric two-column — dark copy panel on one side, diamond-masked photo on the other
- **Feature grid**: 2–3 column grid on desktop, single column on mobile
- **Professor grid**: evenly spaced row of circular portraits, centered

### Whitespace Philosophy
Because the canvas is dark, whitespace here reads as **depth** rather than emptiness — generous vertical padding between sections lets each block of Spark-accented content feel like its own spotlighted moment against the black, rather than a continuous dense feed.

### Border Radius Scale

| Radius | Use |
|--------|-----|
| 12–16px | Small decorative tiles, minor UI accents |
| 16–20px | Curriculum cards, FAQ rows |
| 24–40px | Promotional banners, pricing cards, hero media frames |
| 50% | Professor portraits, small icon buttons |
| 999px | Buttons, badges, nav pills, form inputs — the signature interactive radius |

The scale intentionally skips the 4–8px range: anything sharp-cornered belongs to the brand mark's decorative weave, never to a UI surface.

## 6. Depth & Elevation

| Level | Treatment | Use |
|-------|-----------|-----|
| 0 | No shadow, 1.5–2px Spark border only | Default for cards and portraits — borders are the primary depth cue since shadows barely register on near-black surfaces |
| 1 | `rgba(251,186,0,0.15) 0px 4px 16px` | Subtle hover lift on cards and links |
| 2 | `rgba(251,186,0,0.25) 0px 12px 32px` | Primary CTA buttons, pricing card — a "glow" drawing the eye toward conversion points |
| 3 | `rgba(251,186,0,0.35) 0px 40px 80px` | Rare; used once per page at most, on the hero CTA or the enrollment banner |

### Shadow Philosophy
On a dark canvas, black shadows disappear. Codemaster replaces directional shadow with **Spark-tinted glow**, treating elevation as "how much light this element emits" rather than "how much shadow it casts." Borders remain the default functional delineator; glow is reserved for elements that should visually pull focus.

## 7. Do's and Don'ts

### Do
- Default the page background to Offline Black (`#0F0F0F`) — Codemaster is dark-theme-first
- Use Spark Yellow generously across headlines, borders, icons, and CTAs — it is the hero color, not a restricted accent
- Keep every interactive element (buttons, badges, portrait frames) fully rounded, contrasting the sharp brand mark
- Reserve the chevron/diamond Spark Weave pattern for decoration and photo masking only
- Use thin Spark borders (1.5–2px) as the primary way to delineate cards on dark surfaces
- Apply the Suntech Gradient only to hero-scale promotional blocks
- Set headlines in Readex Pro SemiBold, body copy in Readex Pro Light — never mix the assignment
- Use the Online White bar for navigation as the one intentional "light" surface on the page

### Don't
- Don't use pure white (`#FFFFFF`) as body text — prefer Online White (`#F9F9F9`) for a warmer, on-brand tone
- Don't place the Spark Weave pattern behind readable body copy — it will hurt legibility
- Don't round card corners at 8–12px — commit to either the card scale (16–40px) or the full pill (999px)
- Don't introduce colors outside the yellow/black/white/gradient family (no blues, greens, or purples)
- Don't use black drop shadows — they vanish against Offline Black; use Spark-tinted glow instead
- Don't set long paragraphs in SemiBold — reserve it for headlines, labels, and buttons only
- Don't crop hero or promotional photography into plain rectangles when the diamond mask is available

## 8. Responsive Behavior

### Breakpoints

| Name | Width | Key Changes |
|------|-------|-------------|
| Mobile | ≤ 767px | Nav collapses to logo + hamburger; hero stacks (photo above copy); feature grid becomes single column; H1 drops to ~36px; footer logo lockup and pattern strip stay centered but simplified |
| Tablet | 768–1023px | Nav shows 2–3 primary links; feature grid arranges 2-up; hero headline ~44px |
| Desktop | ≥ 1024px | Full nav bar; feature grid 3-up; hero two-column with diamond-masked photo; H1 at 56px |
| Wide | ≥ 1440px | Content max-width caps at ~1280px; gutters grow symmetrically |

### Touch Targets
All interactive elements meet or exceed 44×44px. Pill buttons run 44–52px tall depending on padding; the mobile hamburger and any icon-only controls are 44×44px minimum.

### Collapsing Strategy
- **Nav**: full link row → hamburger overlay, logo lockup always visible
- **Hero**: two-column → stacked, with the diamond-masked photo moving above the copy on mobile
- **Feature grid**: 3-up → 2-up → 1-up
- **Professor grid**: row → wrapped grid → single column
- **Footer**: centered lockup and pattern strip remain full-width and centered at every breakpoint, simplifying only the fine-print rows

### Image Behavior
Diamond-masked photography scales proportionally, preserving its silhouette at every size. Circular professor portraits shrink but always remain perfect circles with their Spark ring intact.

## 9. Agent Prompt Guide

### Quick Color Reference
- Primary canvas: "Offline Black (`#0F0F0F`) — the default dark page background"
- Primary accent / CTA: "Spark Yellow (`#FBBA00`) — headlines, borders, icons, primary buttons"
- Light surface: "Online White (`#F9F9F9`) — used for the navigation bar and any intentional light section"
- Gradient: "Suntech Gradient (`#FBBA00 → #DB8800`) — reserved for hero-scale promotional banners"
- Text on dark: "Online White (`#F9F9F9`)"
- Text on light: "Offline Black (`#0F0F0F`)"
- Muted text: "Fog Gray (`#B3B3B3`)"
- Card fill: "Raised Offline (`#171717`)"
- Footer background: "Deep Offline (`#050505`)"

### Example Component Prompts
- "Create a primary CTA button: Spark Yellow (`#FBBA00`) background, Offline Black (`#0F0F0F`) text, 999px border-radius (full pill), 12px vertical and 28px horizontal padding, Readex Pro SemiBold at 15px."
- "Design a professor portrait: a circular photo 150px in diameter with a 2px Spark Yellow ring border. Below it, add the name in Readex Pro SemiBold Spark Yellow, and beneath that a small pill badge with a 1px Spark border and Spark text reading a credential label."
- "Build a curriculum card: Raised Offline (`#171717`) background, 1.5px Spark Yellow border, 20px border-radius, 28px padding. Title in Readex Pro SemiBold Spark Yellow at 20px, body copy in Readex Pro Light Online White at 16px."
- "Create a promotional banner: full-width rounded rectangle (32px radius) filled with the Suntech Gradient from Spark Yellow to Ember Orange, containing a white logo lockup, a headline in Readex Pro SemiBold Online White, and a diamond-masked photograph on the right side."
- "Design a footer: Deep Offline (`#050505`) background, centered dual-brand logo lockup, minimal link rows in Readex Pro Light Fog Gray at 14px, and a thin decorative strip of the interlocking Spark Weave chevron pattern running along the very bottom edge."

### Iteration Guide
1. Focus on one component at a time.
2. Reference color names alongside hex codes from this document.
3. Describe shapes using the brand vocabulary ("Spark pill," "diamond mask," "Spark Weave") alongside pixel values.
4. Pair every prompt with the intended feel (energetic, technical, mentoring) alongside measurements.
5. When unsure of a radius, default to one of three values: 999px (buttons/badges), 16–20px (cards), or 24–40px (hero/banner blocks).
6. Default backgrounds to Offline Black — switching a section to Online White should always be an intentional, sparing choice, not the default.

### Known Gaps
- Exact intermediate neutrals (grays used for muted text, dividers) are not part of the original four brand colors and were derived here to complete the system — treat them as flexible support tones, not locked brand values.
- The extent to which the Online White "light mode" register should be used beyond the navigation bar is not fully specified by the source assets and may need art-direction judgment per section.
- Tablet-specific spacing was inferred from desktop and mobile proportions and may need fine-tuning per component.
- Readex Pro is an open Google Font, so no proprietary substitute is required — SemiBold and Light are both freely available in the standard font file.