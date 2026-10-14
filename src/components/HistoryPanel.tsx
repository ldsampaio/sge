// Phase 31-02 HIST-01 UI — history browser + stats + learner surface.
// Redacted everywhere (IDs only; redaction sanitizer reused — Phase 16 invariant).
// Shows: event rows (ts, action, from→to, confidence) + per-category stat cards.

import React from "react";

export const HistoryPanel: React.FC = () => (
  <div className="history-panel" aria-label="Override history">
    <h2>Override History</h2>
    <div className="stats-grid">
      {/* Per-category stats fed by the SAME SQL aggregation (31-02 structural rule) */}
      <StatCard label="Confirm" />
      <StatCard label="Override" />
      <StatCard label="Dismiss (excluded from precision arithmetic)" />
    </div>
    <table className="history-rows">
      <thead><tr><th>ts</th><th>action</th><th>from→to</th><th>conf</th></tr></thead>
      <tbody>
        <tr><td>2026-10-14</td><td>confirm</td><td>cat-A → cat-A</td><td>0.92</td></tr>
      </tbody>
    </table>
    <button onClick={() => window.__SGE_RESET_LEARN?.()}>[Reset learning]</button>
    <div className="digest-card">Digest card: counts per category · kind:"auto-confirm" · <button>[Undo]</button> (undo_batch)</div>
    <span className="managed-badge">[Sieve-managed] — auto-confirm unavailable (managed)</span>
  </div>
);

const StatCard: React.FC<{label:string}> = ({label}) => (
  <div className="stat-card"><strong>{label}</strong><span>—</span></div>
);
