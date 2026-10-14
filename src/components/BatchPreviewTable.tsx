/** Phase 30 trust-pair UI: shared per-item preview table (BatchPanel +
 *  ArchivePanel render the SAME shape — never a second preview
 *  component). All strings are backend-enum/label-ID cents (basis,
 *  skipped_reason from a fixed set) — never bodies; the backend
 *  redaction-sanitizes every row before it crosses IPC. */

export interface PreviewItem {
  message_id: number;
  from_folder: string;
  to_folder: string;
  dest_display: string;
  basis: string;
  skipped_reason: string | null;
}

export interface BatchPreview {
  kind: string;
  kind_tag: string;
  items: PreviewItem[];
  skipped: PreviewItem[];
  total_candidates: number;
  truncated: boolean;
  remainder: number;
}

interface BatchPreviewTableProps {
  preview: BatchPreview;
  /** [Run for real] — calls the fresh run command (never replays rows). */
  onConfirm: () => void;
  confirming: boolean;
  confirmLabel: string;
}

const SKIPPED_REASON_PT: Record<string, string> = {
  "review-bucket": "para revisão",
  excluded: "excluída",
  undatable: "sem data",
  pinned: "correção fixada",
};

export function BatchPreviewTable({ preview, onConfirm, confirming, confirmLabel }: BatchPreviewTableProps) {
  const shown = preview.items.slice(0, 50);
  return (
    <div style={{ marginTop: 12, borderTop: "1px solid #1e293b", paddingTop: 12 }}>
      <h3 style={{ margin: "0 0 4px" }}>
        Pré-visualização{preview.kind_tag === "archive" ? " de arquivo" : ""}
      </h3>
      <p style={{ color: "#94a3b8", fontSize: "0.8rem" }}>
        {preview.total_candidates} para mover · {preview.skipped.length} ignorados
        {preview.remainder > 0 && ` · restam ${preview.remainder} para revisão`}
        {preview.truncated && " · lista limitada a 500 (valores totais acima)"}
      </p>
      <table style={{ width: "100%", fontSize: "0.8rem", borderCollapse: "collapse" }}>
        <thead>
          <tr style={{ textAlign: "left", color: "#94a3b8" }}>
            <th>De</th>
            <th>Para</th>
            <th>Base</th>
          </tr>
        </thead>
        <tbody>
          {shown.map((it) => (
            <tr key={it.message_id} style={{ borderTop: "1px solid #1e293b" }}>
              <td>{it.from_folder}</td>
              <td>{it.dest_display || it.to_folder}</td>
              <td style={{ color: "#94a3b8" }}>{it.basis}</td>
            </tr>
          ))}
        </tbody>
      </table>
      {preview.items.length > shown.length && (
        <p style={{ color: "#94a3b8", fontSize: "0.8rem" }}>
          …e mais {preview.items.length - shown.length} na lista completa do relatório.
        </p>
      )}
      {preview.skipped.length > 0 && (
        <>
          <h4 style={{ margin: "8px 0 4px", fontSize: "0.85rem" }}>Ignorados</h4>
          <ul style={{ listStyle: "none", padding: 0, fontSize: "0.8rem", color: "#94a3b8" }}>
            {preview.skipped.slice(0, 20).map((s) => (
              <li key={s.message_id}>
                {s.from_folder} · {SKIPPED_REASON_PT[s.skipped_reason ?? ""] ?? s.skipped_reason}
              </li>
            ))}
            {preview.skipped.length > 20 && <li>…e mais {preview.skipped.length - 20}</li>}
          </ul>
        </>
      )}
      <div style={{ marginTop: 8 }}>
        <button type="button" className="btn btn-small" disabled={confirming} onClick={onConfirm}>
          {confirmLabel}
        </button>
      </div>
    </div>
  );
}
