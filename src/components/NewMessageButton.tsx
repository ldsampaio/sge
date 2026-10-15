// Phase 37 — Compose UI / New Message Button (v1.7 milestone)
// Toolbar button: opens ComposeWindow; no SMTP at create time
// Reuses Phase 14 contract; wire to queue_send (M10) via ComposeWindow
// BCC envelope-only preserved (Phase 35); zero new core dependencies
// BODY.PEEK-only invariant preserved; UID-only addressing preserved

import React from "react";
export const NewMessageButton: React.FC<{ onClick: () => void }> = ({ onClick }) => (
  <button onClick={onClick} aria-label="New Message" className="toolbar-btn new-message-btn">
    New Message
  </button>
);
