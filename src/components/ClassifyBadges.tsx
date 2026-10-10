import React from "react";

/** Phase 18 trust badges: primary category chip + distinct local-only
 *  secondary chip. Review-routed mail gets a dashed outline (needs eyes,
 *  not filing). */

interface CategoryBadgeProps {
  primary: string;
  secondary?: string | null;
  confidence?: number;
  needsReview?: boolean;
}

export function CategoryBadge({ primary, secondary, confidence, needsReview }: CategoryBadgeProps) {
  return (
    <span style={{ display: "inline-flex", gap: 4, alignItems: "center" }} aria-label={`categoria ${primary}`}>
      <span
        className="category-badge"
        title={confidence != null ? `confiança ${Math.round(confidence * 100)}%` : primary}
        style={{
          fontSize: "0.68rem",
          fontWeight: 600,
          padding: "1px 7px",
          borderRadius: 999,
          background: needsReview ? "transparent" : "#1d4ed8",
          color: needsReview ? "#93c5fd" : "#fff",
          border: needsReview ? "1px dashed #3b82f6" : "1px solid transparent",
          whiteSpace: "nowrap",
        }}
      >
        {primary}
      </span>
      {secondary && (
        <span
          className="category-badge-secondary"
          title="rótulo local — não move e-mails"
          style={{
            fontSize: "0.68rem",
            padding: "1px 7px",
            borderRadius: 999,
            background: "transparent",
            color: "#94a3b8",
            border: "1px dotted #64748b",
            whiteSpace: "nowrap",
          }}
        >
          {secondary}
        </span>
      )}
    </span>
  );
}

/** Suggestion bar for the reading pane: destination + confidence +
 *  redacted justification + Move/Correct/Dismiss. Single-email moves
 *  ALWAYS pass through here (backend refuses label-less confirms). */
export interface TopsOption {
  id: string;
  name: string;
}

interface SuggestionBarProps {
  destDisplay: string;
  confidence: number;
  needsReview: boolean;
  justification: string;
  tops: TopsOption[];
  busy: boolean;
  message: string | null;
  onConfirm: () => void;
  onCorrect: (toId: string) => void;
  onDismiss: () => void;
}

export function SuggestionBar({
  destDisplay,
  confidence,
  needsReview,
  justification,
  tops,
  busy,
  message,
  onConfirm,
  onCorrect,
  onDismiss,
}: SuggestionBarProps) {
  const [correcting, setCorrecting] = React.useState(false);
  const pct = Math.round(confidence * 100);
  return (
    <div
      className="suggestion-bar"
      role="group"
      aria-label="Sugestão de arquivamento"
      style={{
        margin: "8px 0",
        padding: "8px 12px",
        borderRadius: 8,
        border: needsReview ? "1px dashed #3b82f6" : "1px solid #1e40af",
        background: "rgba(29, 78, 216, 0.08)",
        fontSize: "0.82rem",
      }}
    >
      <div style={{ display: "flex", gap: 8, alignItems: "center", flexWrap: "wrap" }}>
        <span>
          {needsReview ? "Revisão sugerida" : "Arquivar em"} <strong>{destDisplay}</strong>
          <span style={{ color: "#94a3b8" }}> · {pct}%</span>
        </span>
        {!correcting ? (
          <>
            <button type="button" className="btn btn-small" disabled={busy} onClick={onConfirm}>
              Mover
            </button>
            <button type="button" className="btn btn-small" disabled={busy} onClick={() => setCorrecting(true)}>
              Corrigir
            </button>
            <button type="button" className="btn btn-small" disabled={busy} onClick={onDismiss}>
              Dispensar
            </button>
          </>
        ) : (
          <>
            <select
              aria-label="Categoria correta"
              disabled={busy}
              defaultValue=""
              onChange={(e) => {
                if (e.target.value) {
                  onCorrect(e.target.value);
                  setCorrecting(false);
                }
              }}
            >
              <option value="" disabled>
                Escolha a categoria…
              </option>
              {tops.map((t) => (
                <option key={t.id} value={t.id}>
                  {t.name}
                </option>
              ))}
            </select>
            <button type="button" className="btn btn-small" onClick={() => setCorrecting(false)}>
              Voltar
            </button>
          </>
        )}
      </div>
      {justification && (
        <div style={{ color: "#94a3b8", marginTop: 4 }}>{justification}</div>
      )}
      {message && (
        <div role="status" style={{ marginTop: 4, color: "#eab308" }}>
          {message}
        </div>
      )}
    </div>
  );
}
