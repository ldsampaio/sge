import React from "react";
import { invoke } from "@tauri-apps/api/core";
import type { SendStatusResult } from "../types";

/** Minimal badge beside SyncStatus — shows pending send count from send_status.
 *  Hidden at zero; uses the pt-BR tone from SyncStatus copy. */
interface OutboxBadgeProps {
  /** Visible only when pending_count > 0 */
  visible: boolean;
  /** Pending send count from send_status */
  pendingCount: number;
  /** pt-BR copy for the pending line */
  copy: string;
}

/** OutboxBadge renders a small badge with the pending send count.
 *  The caller positions it beside SyncStatus; this component is pure display.
 *  Hidden when pendingCount is 0. */
function OutboxBadge({ visible, copy }: OutboxBadgeProps) {
  if (!visible) return null;
  return (
    <div
      className="outbox-badge"
      aria-label="outbox pendente"
      role="status"
      style={{
        display: "inline-flex",
        alignItems: "center",
        marginLeft: 8,
        color: "#eab308",
        fontSize: "0.75rem",
        fontWeight: 500,
      }}
    >
      {copy}
    </div>
  );
}

/** Wired component: fetches send_status and renders the OutboxBadge. */
interface OutboxBadgeWiredProps {
  mailbox?: string;
}
export function OutboxBadgeWired({ mailbox = "INBOX" }: OutboxBadgeWiredProps) {
  const [sendStatus, setSendStatus] = React.useState<SendStatusResult | null>(null);

  // Fetch send_status on mount and whenever mailbox changes.
  React.useEffect(() => {
    ;(async () => {
      try {
        const result: SendStatusResult = await invoke(
          "send_status",
          { mailbox }
        ) as SendStatusResult;
        setSendStatus(result);
      } catch (e) {
        console.error("[SGE] send_status invoke failed:", e);
      }
    })();
  }, [mailbox]);

  // pt-BR copy: when there are pending sends, show the count line;
  // otherwise the badge is hidden (visible=false) so nothing renders.
  const badgeVisible = (sendStatus?.pending_count ?? 0) > 0;
  const copy = sendStatus
    ? `${sendStatus.pending_count} mensagem${sendStatus.pending_count === 1 ? "" : "s"} pendentes`
    : "";

  return badgeVisible ? (
    <OutboxBadge
      visible={badgeVisible}
      pendingCount={sendStatus?.pending_count || 0}
      copy={copy}
    />
  ) : null;
}