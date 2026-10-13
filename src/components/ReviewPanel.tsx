import React from "react";
import { invoke } from "@tauri-apps/api/core";

/** Phase 18 review surface: `A Classificar` queue with per-item
 *  confirm/dismiss + the confidence threshold slider (future suggestions
 *  only — history is never relabeled). Rendered as a modal from the
 *  sidebar row. */

interface ReviewItem {
  message_id: number;
  primary_id: string;
  secondary_id: string | null;
  confidence: number;
  threshold: number;
  stale: boolean;
  needs_review: boolean;
}

/** Phase 25 rule hit: deterministic suggestion with transient redacted
 *  justification, overlap warning, and the classifier alternative when
 *  both exist (user picks — dismissal keeps mail in place). */
interface RuleHit {
  message_id: number;
  rule_id: string;
  rule_name: string;
  dest_kind: string;
  destination: string;
  justification: string;
  overlap_note: string | null;
  classifier_alternative: {
    category_id: string;
    category_name: string;
    confidence: number;
  } | null;
}

interface ReviewPanelProps {
  onClose: () => void;
  onChanged: () => void;
}

export function ReviewPanel({ onClose, onChanged }: ReviewPanelProps) {
  const [items, setItems] = React.useState<ReviewItem[] | null>(null);
  const [ruleHits, setRuleHits] = React.useState<RuleHit[] | null>(null);
  const [threshold, setThreshold] = React.useState<number>(0.6);
  const [busy, setBusy] = React.useState(false);
  const [message, setMessage] = React.useState<string | null>(null);

  const reload = React.useCallback(async () => {
    try {
      const rows = (await invoke("review_list", { limit: 100 })) as ReviewItem[];
      setItems(rows);
    } catch (e) {
      setMessage(`Falha ao carregar a revisão: ${e}`);
    }
    try {
      const hits = (await invoke("pending_rule_suggestions", { limit: 100 })) as RuleHit[];
      setRuleHits(hits);
    } catch (e) {
      setMessage(`Falha ao carregar sugestões de regras: ${e}`);
    }
  }, []);

  React.useEffect(() => {
    void reload();
  }, [reload]);

  const confirm = async (messageId: number) => {
    setBusy(true);
    setMessage(null);
    try {
      const r = (await invoke("confirm_suggestion", {
        messageId,
        allowNest: false,
      })) as { detail: string };
      setMessage(r.detail);
      onChanged();
      await reload();
    } catch (e) {
      const s = String(e);
      if (s.includes("COLLISION")) {
        if (window.confirm("Já existe uma pasta Auto. Arquivar dentro dela?")) {
          try {
            const r2 = (await invoke("confirm_suggestion", {
              messageId,
              allowNest: true,
            })) as { detail: string };
            setMessage(r2.detail);
            onChanged();
            await reload();
          } catch (e2) {
            setMessage(`Falha ao arquivar: ${e2}`);
          }
        }
      } else {
        setMessage(`Falha ao arquivar: ${e}`);
      }
    } finally {
      setBusy(false);
    }
  };

  const dismiss = async (messageId: number) => {
    try {
      await invoke("dismiss_suggestion", { messageId });
      onChanged();
      await reload();
    } catch (e) {
      setMessage(`Falha ao dispensar: ${e}`);
    }
  };

  /** Confirm one rule hit. `pick` resolves rule-vs-classifier conflicts:
   *  "rule" moves to the rule destination, "classifier" to the suggested
   *  category; dismissal (above) keeps mail in place. Overlap destinations
   *  ("gerenciada pelo classificador") reuse the same confirm path — the
   *  Auto-collision dialog appears when the tree needs nesting. */
  const confirmRule = async (messageId: number, pick: "rule" | "classifier") => {
    setBusy(true);
    setMessage(null);
    try {
      const r = (await invoke("confirm_suggestion", {
        messageId,
        allowNest: false,
        prefer: pick === "classifier" ? "classifier" : null,
      })) as { detail: string };
      setMessage(r.detail);
      onChanged();
      await reload();
    } catch (e) {
      const s = String(e);
      if (s.includes("COLLISION")) {
        if (window.confirm("Já existe uma pasta Auto. Arquivar dentro dela?")) {
          try {
            const r2 = (await invoke("confirm_suggestion", {
              messageId,
              allowNest: true,
              prefer: pick === "classifier" ? "classifier" : null,
            })) as { detail: string };
            setMessage(r2.detail);
            onChanged();
            await reload();
          } catch (e2) {
            setMessage(`Falha ao arquivar: ${e2}`);
          }
        }
      } else {
        setMessage(`Falha ao arquivar: ${e}`);
      }
    } finally {
      setBusy(false);
    }
  };

  const saveThreshold = async (v: number) => {
    setThreshold(v);
    try {
      await invoke("set_confidence_threshold", { value: v });
    } catch (e) {
      setMessage(`Falha ao salvar o limiar: ${e}`);
    }
  };

  return (
    <div
      role="dialog"
      aria-modal="true"
      aria-label="A Classificar"
      style={{
        position: "fixed",
        inset: 0,
        background: "rgba(0,0,0,0.5)",
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
        zIndex: 60,
      }}
      onClick={onClose}
    >
      <div
        style={{
          background: "var(--color-bg, #0f172a)",
          borderRadius: 12,
          padding: 20,
          minWidth: 420,
          maxWidth: 640,
          maxHeight: "80vh",
          overflow: "auto",
        }}
        onClick={(e) => e.stopPropagation()}
      >
        <h2 style={{ marginTop: 0 }}>A Classificar</h2>
        <p style={{ color: "#94a3b8", fontSize: "0.85rem" }}>
          E-mails que o classificador não teve certeza — nada aqui se move sozinho.
        </p>
        <label style={{ display: "block", margin: "12px 0" }}>
          Limiar de confiança: <strong>{Math.round(threshold * 100)}%</strong>
          <input
            type="range"
            min={0}
            max={100}
            value={Math.round(threshold * 100)}
            onChange={(e) => void saveThreshold(Number(e.target.value) / 100)}
            style={{ width: "100%" }}
          />
        </label>
        {message && (
          <div role="status" style={{ color: "#eab308", marginBottom: 8 }}>
            {message}
          </div>
        )}
        {items === null ? (
          <p>Carregando…</p>
        ) : items.length === 0 ? (
          <p>Nada para revisar. 🎉</p>
        ) : (
          <ul style={{ listStyle: "none", padding: 0 }}>
            {items.map((it) => (
              <li
                key={it.message_id}
                style={{
                  display: "flex",
                  gap: 8,
                  alignItems: "center",
                  padding: "6px 0",
                  borderTop: "1px solid #1e293b",
                }}
              >
                <span style={{ flex: 1 }}>
                  {it.primary_id} <span style={{ color: "#94a3b8" }}>{Math.round(it.confidence * 100)}%</span>
                </span>
                <button type="button" className="btn btn-small" disabled={busy} onClick={() => void confirm(it.message_id)}>
                  Mover
                </button>
                <button type="button" className="btn btn-small" disabled={busy} onClick={() => void dismiss(it.message_id)}>
                  Dispensar
                </button>
              </li>
            ))}
          </ul>
        )}
        <h3 style={{ margin: "16px 0 4px" }}>Sugestões de regras</h3>
        <p style={{ color: "#94a3b8", fontSize: "0.8rem", margin: "0 0 8px" }}>
          Destinos determinísticos das suas regras — nada se move sem confirmar.
        </p>
        {ruleHits === null ? (
          <p>Carregando…</p>
        ) : ruleHits.length === 0 ? (
          <p style={{ fontSize: "0.85rem" }}>Nenhuma sugestão de regra pendente.</p>
        ) : (
          <ul style={{ listStyle: "none", padding: 0 }}>
            {ruleHits.map((h) => (
              <li
                key={h.message_id}
                style={{ padding: "8px 0", borderTop: "1px solid #1e293b" }}
              >
                <div style={{ display: "flex", gap: 8, alignItems: "center" }}>
                  <span style={{ flex: 1 }}>
                    <strong>{h.rule_name}</strong> → {h.destination}
                  </span>
                  <button type="button" className="btn btn-small" disabled={busy} onClick={() => void confirmRule(h.message_id, "rule")}>
                    Mover
                  </button>
                  {h.classifier_alternative && (
                    <button
                      type="button"
                      className="btn btn-small"
                      disabled={busy}
                      title={`Classificador sugere ${h.classifier_alternative.category_name}`}
                      onClick={() => void confirmRule(h.message_id, "classifier")}
                    >
                      Usar {h.classifier_alternative.category_name}
                    </button>
                  )}
                  <button type="button" className="btn btn-small" disabled={busy} onClick={() => void dismiss(h.message_id)}>
                    Dispensar
                  </button>
                </div>
                {h.justification && (
                  <div style={{ color: "#94a3b8", fontSize: "0.8rem", marginTop: 4 }}>{h.justification}</div>
                )}
                {h.classifier_alternative && (
                  <div style={{ color: "#eab308", fontSize: "0.8rem", marginTop: 4 }}>
                    O classificador sugere {h.classifier_alternative.category_name} (
                    {Math.round(h.classifier_alternative.confidence * 100)}%) — escolha um destino.
                  </div>
                )}
                {h.overlap_note && (
                  <div style={{ color: "#eab308", fontSize: "0.8rem", marginTop: 4 }}>
                    {h.overlap_note}
                  </div>
                )}
              </li>
            ))}
          </ul>
        )}
        <div style={{ marginTop: 12, textAlign: "right" }}>
          <button type="button" className="btn" onClick={onClose}>
            Fechar
          </button>
        </div>
      </div>
    </div>
  );
}
