---
phase: 03
slug: mailbox-ui-shell-search
status: approved
shadcn_initialized: false
preset: none
created: 2026-10-03
---

# Phase 3 — UI Design Contract

> Visual and interaction contract for the Mailbox UI Shell + Search.

---

## Design System

| Property | Value |
|----------|-------|
| Tool | none |
| Preset | not applicable |
| Component library | none (plain React + CSS) |
| Icon library | none (CSS-only unread dot, Unicode for empty states) |
| Font | Inter (already in App.css `:root`) |

**No new dependencies.** The existing stack uses raw React 19 + Vite + CSS. No shadcn/ui, no Radix, no icon library. The unread indicator is a CSS pseudo-element dot. Folder/iconography uses Unicode characters (📭 for INBOX).

---

## Spacing Scale

Declared values (must be multiples of 4):

| Token | Value | Usage |
|-------|-------|-------|
| xs | 4px | Icon gaps, inline padding |
| sm | 8px | Compact element spacing |
| md | 16px | Default element spacing |
| lg | 24px | Section padding |
| xl | 32px | Layout gaps |
| 2xl | 48px | Major section breaks |

Exceptions: none

---

## Typography

| Role | Size | Weight | Line Height |
|------|------|--------|-------------|
| Body | 14px | 400 | 1.5 |
| Label | 12px | 500 | 1.4 |
| Heading | 16px | 600 | 1.3 |
| Display | 20px | 600 | 1.25 |

---

## Color

| Role | Value | Usage |
|------|-------|-------|
| Dominant (60%) | #f6f6f6 (light) / #2a2a2a (dark) | Page background |
| Secondary (30%) | #ffffff (light) / #1a1a1a (dark) | Sidebar, list rows, reading pane |
| Accent (10%) | #646cff (light) / #24c8db (dark) | Selected row highlight, INBOX node |
| Destructive | #e74c3c | Not used in M1 (no destructive actions) |
| Unread dot | #4f77d6 | Unread indicator dot |
| Read dot | transparent | Read indicator (invisible) |

**Accent reserved for:** selected message row background, INBOX node active state, sync progress bar.

**Color scheme:** Reuses existing App.css `:root` and `prefers-color-scheme: dark` variables. No new color tokens.

---

## Layout

**Three-pane Gmail-like layout:**

```
┌─────────────────────────────────────────────────────────┐
│  Sidebar        │  Message List        │  Reading Pane   │
│  ┌───────────┐  │  ┌─────────────────┐  │  ┌───────────┐  │
│  │ 📭 INBOX   │  │  │ Search [______]│  │  │           │  │
│  │           │  │  │ UID 1234       │  │  │ Select a  │  │
│  │ (240px)   │  │  │ Sender name    │  │  │ message   │  │
│  └───────────┘  │  │ Subject line   │  │  │ to read   │  │
│                 │  │ Today 10:30 AM │  │  │           │  │
│                 │  │ ●              │  │  │           │  │
│                 │  └─────────────────┘  │  └───────────┘  │
│  ───────────────┼  ────────────────────┼  ───────────────┤
│  Width: 240px   │  Flex-grow           │  Width: 320px   │
└─────────────────────────────────────────────────────────┘
```

- **Sidebar:** Fixed 240px width. INBOX node with message count badge.
- **Message List:** Flex-grow, scrollable vertical. Fixed header with search bar.
- **Reading Pane:** Fixed 320px width. Placeholder content in Phase 3.
- **Overall:** `display: flex` on `.mailbox-layout`, `overflow: hidden` on panes.

---

## Copywriting Contract

| Element | Copy |
|---------|------|
| Primary CTA | Sync Now |
| Empty state heading | No messages in INBOX |
| Empty state body | Connect to your server and sync to see messages here. |
| Error state | Failed to load messages: {detail}. Retry |
| Loading state | Loading messages… |
| Search placeholder | Search sender, subject… |
| Reading pane placeholder | Select a message to read |
| Unread dot (aria-label) | Unread |

---

## Interaction Contract

### Message List Row
- Click a row to select (highlight with accent background).
- Show: from_addr (bold if unread), subject, date (humanized: "Today 10:30 AM"), has_attachments (📎 Unicode icon if true).
- Unread indicator: filled dot (●) for unread, empty space for read.
- Hover state: light gray background.
- Selected state: accent background, slight inset.

### Search
- Search bar always visible in message list header.
- 300ms debounce before firing `search_messages` command.
- Clear button (×) in search input when non-empty.
- Empty search returns to paginated `list_messages` view.
- Results shown in same list area (replace mode, not filter mode).

### States
| Pane | Loading | Empty | Error |
|------|---------|-------|-------|
| Sidebar | — | — | — |
| Message List | "Loading messages…" skeleton | "No messages in INBOX" + sync hint | "Failed to load messages: {detail}" + Retry |
| Reading Pane | — | "Select a message to read" | — |

---

## Checker Sign-Off

- [x] Dimension 1 Copywriting: PASS
- [x] Dimension 2 Visuals: PASS
- [x] Dimension 3 Color: PASS
- [x] Dimension 4 Typography: PASS
- [x] Dimension 5 Spacing: PASS
- [x] Dimension 6 Registry Safety: PASS

**Approval:** approved 2026-10-03
