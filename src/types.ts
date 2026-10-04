// Shared types for the mailbox UI (Phase 3).
// Mirrors the Rust MessageRow struct from store/queries.rs.

export interface MessageRow {
  uid: number;
  subject: string;
  from_addr: string;
  to_addrs: string;
  date_utc: string;
  flags: string;
  has_attachments: boolean;
  preview: string;
}

export interface AttachmentInfo {
  name: string;
  size: number;
  content_type: string;
  part_number: string;
}

export interface MessageView {
  uid: number;
  subject: string;
  from_addr: string;
  to_addrs: string[];
  date_utc: string;
  html: string | null;
  text: string | null;
  has_attachments: boolean;
  attachments: AttachmentInfo[];
}

export interface SyncStatusInfo {
  mailbox: string;
  last_sync_at: string;
  uid_validity: number;
  uid_next: number;
  message_count: number;
  /** Durable-outbox depth: toggles awaiting server acknowledgement (FLAG-02). */
  pending_count: number;
}

/**
 * Outcome of the `set_seen` Tauri command (Phase 6 backend).
 * The toggle applies locally instantly; `acked` tells whether the server
 * confirmed the write or the op stays queued, with `pending_count` shown
 * as the pending indicator until a later sync acknowledges it.
 */
export interface SetSeenResult {
  uid: number;
  seen: boolean;
  acked: boolean;
  pending_count: number;
  detail: string;
}

/** Local UI event bus for flag toggles (list <-> reader -> status line). */
export const FLAG_UPDATE_EVENT = "sge:flag-update";

export interface FlagUpdateDetail {
  uid: number;
  /** Target seen state the toggle applied (or attempted). */
  seen: boolean;
  /** False when the server has not confirmed yet (queued) or the invoke failed. */
  acked: boolean;
  /**
   * Post-op outbox depth from `set_seen`, or -1 when unknown (invoke
   * rejected before the backend could report). Listeners must fall back
   * to their last synced count when negative; -1 also marks error
   * notices the list must not apply as row state.
   */
  pending_count: number;
  /** Human-readable detail; doubles as the failure-line title text. */
  detail: string;
  origin: "list" | "reader";
}

export function dispatchFlagUpdate(detail: FlagUpdateDetail): void {
  window.dispatchEvent(new CustomEvent<FlagUpdateDetail>(FLAG_UPDATE_EVENT, { detail }));
}

/**
 * Return a flags JSON string with the \\Seen flag set or cleared,
 * preserving any other flags. Mirrors the optimistic local write in
 * the Phase 6 backend (`set_local_seen`).
 */
export function setSeenInFlags(flags: string, seen: boolean): string {
  try {
    const parsed: unknown = JSON.parse(flags);
    const rest = Array.isArray(parsed)
      ? parsed.filter((f: unknown) => f !== "\\Seen")
      : [];
    if (seen) rest.push("\\Seen");
    return JSON.stringify(rest);
  } catch {
    return seen ? '["\\\\Seen"]' : "[]";
  }
}

/**
 * Returns true when the flags JSON string does NOT contain "\\Seen"
 * — i.e. the message is unread. Mirrors queries::is_unread in Rust.
 */
export function isUnread(flags: string): boolean {
  try {
    const parsed = JSON.parse(flags);
    return !(Array.isArray(parsed) && parsed.some((f: string) => f === "\\Seen"));
  } catch {
    return true; // treat unparseable flags as unread (safe default)
  }
}

/**
 * Date/time for a message-list row: time only ("14:30") for today's mail,
 * full date ("04/10/2026") otherwise. Times use the local timezone.
 */
export function formatRowDate(date_utc: string): string {
  const d = new Date(date_utc);
  if (Number.isNaN(d.getTime())) return date_utc;
  const now = new Date();
  const sameDay =
    d.getFullYear() === now.getFullYear() &&
    d.getMonth() === now.getMonth() &&
    d.getDate() === now.getDate();
  if (sameDay) {
    return d.toLocaleTimeString("pt-BR", { hour: "2-digit", minute: "2-digit" });
  }
  return d.toLocaleDateString("pt-BR", {
    day: "2-digit",
    month: "2-digit",
    year: "numeric",
  });
}
