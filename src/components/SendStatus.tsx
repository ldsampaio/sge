import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { SendStatusResult } from "../types";

interface SendStatusProps {
  mailbox?: string;
  onRetry?: (queueId: string) => void;
}

type Status =
  | { kind: "idle" }
  | { kind: "synced"; status: SendStatusResult }
  | { kind: "uncertain"; explanation: string };

export default function SendStatus({ mailbox = "INBOX", onRetry }: SendStatusProps) {
  const [status, setStatus] = useState<Status>({ kind: "idle" });
  const [failedEntries, setFailedEntries] = useState<Array<{ queueId: string; reason: string }>>(
    [],
  );

  useEffect(() => {
    ;(async () => {
      try {
        const result: SendStatusResult = await invoke("send_status", { mailbox }) as SendStatusResult;
        setStatus({ kind: "synced", status: result });

        const rows: Array<{ queueId: string; reason: string }> = [];
        if (result.failed > 0) {
          rows.push(
            ...(result.pending_count > 0
              ? [{ queueId: "pending-failed", reason: "Falha de envio — tap retry para reenviar" }]
              : []),
          );
        }
        setFailedEntries(rows);
      } catch (e) {
        console.error("[SGE] send_status invoke failed:", e);
        setStatus({ kind: "idle" });
      }
    })();
  }, [mailbox]);

  const uncertainLine = status.kind === "uncertain" ? (
    <p className="unsent-line" role="status">
      Reconciliando na pasta Enviadas — nenhum novo envio será feito até
      a próxima verificação.
    </p>
  ) : null;

  const retryButton = (queueId: string) => (
    <button className="retry-btn" onClick={() => onRetry?.(queueId)} disabled={!onRetry}>
      Retentar
    </button>
  );

  return (
    <div className="send-status" aria-label="status de envio">
      {status.kind === "idle" && (
        <p>Ainda não verificado — dispare um envio para ver o status.</p>
      )}

      {status.kind === "synced" && (
        <>
          {status.status.pending_count > 0 && (
            <p className="pending-line" role="status">
              {status.status.pending_count === 1
                ? "1 alteração aguardando envio"
                : `${status.status.pending_count} alterações aguardando envio`}
            </p>
          )}

          {failedEntries.length > 0 && (
            <div className="failed-surface">
              <p className="failed-title" role="alert">
                {failedEntries.length === 1 ? "1 mensagem com falha" : `${failedEntries.length} mensagens com falha`}
              </p>
              <div className="failed-list">
                {failedEntries.map((entry, i) => (
                  <div key={i} className="failed-item">
                    <span className="failed-reason">{entry.reason}</span>
                    {onRetry && retryButton(entry.queueId)}
                  </div>
                ))}
              </div>
            </div>
          )}

          {uncertainLine}
        </>
      )}

      {status.kind === "idle" && (
        <button
          className="refresh-btn"
          onClick={() => {
            ;(async () => {
              try {
                const result: SendStatusResult = await invoke("send_status", { mailbox }) as SendStatusResult;
                setStatus({ kind: "synced", status: result });
                const rows: Array<{ queueId: string; reason: string }> = [];
                if (result.failed > 0) {
                  rows.push(
                    ...(result.pending_count > 0
                      ? [{ queueId: "pending-failed", reason: "Falha de envio — tap retry para reenviar" }]
                      : []),
                  );
                }
                setFailedEntries(rows);
              } catch (e) {
                console.error("[SGE] send_status invoke failed:", e);
              }
            })();
          }}
        >
          Atualizar
        </button>
      )}
    </div>
  );
}