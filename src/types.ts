// Shared types for the mailbox UI (Phase 3).
// Mirrors the Rust MessageRow struct from store/queries.rs.

export interface MessageRow {
  uid: number;
  /** Raw wire mailbox name — lets global search jump to the right folder. */
  mailbox: string;
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
  /** Cc recipients — reply seeding. NOT returned by `fetch_message` yet
   *  (`MessageView` in `commands/sync.rs` has no such field), so reply
   *  prefills degrade to empty instead of failing to type-check. */
  cc_addrs?: string[];
  /** Original Message-ID — reply threading (`In-Reply-To`). Backend gap, as above. */
  message_id?: string | null;
  /** Original References chain — reply threading. Backend gap, as above. */
  references?: string | null;
}

/** Cached mailbox row from the local store, surfaced to the sidebar. */
export interface MailboxRow {
  id: number;
  /** Raw wire name (modified UTF-7) — protocol use only, never display. */
  name: string;
  /** Decoded display name for the folder tree. */
  display_name: string;
  /** LIST hierarchy delimiter ('' = flat). */
  delimiter: string;
  /** Resolved role (`inbox|trash|sent|drafts|custom`; '' = not yet resolved). */
  role: string;
  /** Space-joined LIST attributes ('' = none cached). */
  attributes: string;
  uid_validity: number;
  uid_next: number;
  last_sync_at: string | null;
  unread_count: number;
  /** Last STATUS (UNSEEN) datum (M3) — badge fallback for never-synced folders. */
  unseen_count: number;
}

/**
 * Outcome of the `create_folder` Tauri command (Phase 11 backend).
 * The created folder's RAW wire name (for selection) plus the refreshed
 * tree the sidebar re-renders.
 */
export interface FolderTreeResult {
  created: string;
  mailboxes: MailboxRow[];
}

/**
 * Outcome of the `rename_folder` Tauri command: old + new RAW wire names
 * (the UI migrates selection atomically) plus the refreshed tree.
 * `warning` carries the post-rename UIDVALIDITY-bump notice, if any.
 */
export interface RenameFolderResult {
  old: string;
  new: string;
  warning: string | null;
  mailboxes: MailboxRow[];
}

/**
 * Outcome of the `delete_folder` Tauri command: the deleted RAW wire name,
 * the refreshed tree, and the INBOX fallback hint for selection.
 */
export interface DeleteFolderResult {
  deleted: string;
  mailboxes: MailboxRow[];
  fallback: string;
}

/**
 * One compose-session row from the local `drafts` store (Phase 12 backend).
 * Mirrors the Rust `DraftRow` struct from store/queries.rs — field
 * addresses/recipients are comma-separated strings, `dirty` marks content
 * newer than the last acknowledged server copy, `server_uid` is the Phase 13
 * DRAFT-03 send-transaction handoff.
 */
export interface DraftRow {
  id: string;
  mailbox_id: number;
  message_id: string;
  subject: string;
  body: string;
  to: string;
  cc: string;
  bcc: string;
  dirty: boolean;
  server_uid: number | null;
  attachments: string;
  updated_at: string;
}

/**
 * Outcome of the `save_draft` Tauri command (frozen 12-02 contract).
 * The save lands locally instantly; `acked` tells whether the server
 * confirmed the copy or the row stays dirty for the reconnect flush.
 */
export interface DraftSaveResult {
  id: string;
  dirty: boolean;
  server_uid: number | null;
  acked: boolean;
  pending_count: number;
}

/** Outcome of the `discard_draft` Tauri command (frozen 12-02 contract). */
export interface DiscardResult {
  id: string;
  discarded: boolean;
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
    const rest = Array.isArray(parsed) ? parsed.filter((f: unknown) => f !== "\\Seen") : [];
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

export interface SendStatusResult {
  pending_count: number;
  failed: number;
  queued: number;
  sending: number;
  uncertain: number;
  sent_unfiled: number;
}

/** Compose window mode — the `#compose?mode=` values `open_compose_window`
 *  accepts (`commands/window.rs` rejects anything else by name). */
export type ComposeMode = "new" | "reply" | "reply-all";

/** Staged attachment sent with `queue_send`.
 *
 *  NOTE: `queue_send` (Rust) does not accept attachments yet — the extra
 *  field is currently ignored by the backend, so attachments are dropped at
 *  enqueue time. Tracked as a known gap, not a silent success. */
export interface OutgoingAttachmentPayload {
  filename: string;
  content_type: string;
  /** Data URL (`data:<mime>;base64,<payload>`) or bare base64. */
  data_base64: string;
}

/** `queue_send` result — mirrors `send_queue::EnqueueOutcome`. */
export interface QueueSendResult {
  queue_id: string;
  message_id: string;
  /** Send-queue state after enqueue (`queued`). */
  state: string;
  /** True when this Message-ID was already queued (dedupe hit, not a resend). */
  deduped: boolean;
}

/** `send_pending` result — mirrors `commands::sync::SendPendingResult`. */
export interface SendPendingResult {
  /** Relay accepted the bytes and the Sent copy landed. */
  sent: number;
  /** Delivered but the Sent copy is still missing (filing/APPEND leg). */
  sent_unfiled: number;
  failed: number;
  uncertain: number;
  deferred: number;
  /** True when another flush pass held the gate — nothing was attempted. */
  skipped: boolean;
  /** Plain-language reason for the newest parked row (never a body or a key). */
  last_error: string | null;
  /** Resolved Sent mailbox wire name, for an immediate Sent re-sync. */
  sent_wire: string | null;
}

/** `send_row_state` result — mirrors `commands::sync::SendRowStateResult`. */
export interface SendRowState {
  queue_id: string;
  /** `queued|sending|sent|sent_unfiled|failed|uncertain`. */
  state: string;
  attempts: number;
  last_error: string | null;
}

/** Reply context from message cache (BODY.PEEK-only) for threading headers and quote attribution. */
export interface ReplyContext {
  /** Original message's Message-ID for In-Reply-To header. */
  message_id: string;
  /** Original message's References header chain (if any). */
  references: string | null;
  /** Original message's From address. */
  from_addr: string;
  /** Original message's To addresses (comma-separated). */
  to_addrs: string;
  /** Original message's Cc addresses (comma-separated). */
  cc_addrs: string;
  /** Original message's date for quote attribution. */
  date_utc: string;
  /** Original message's text body for quoting (BODY.PEEK read from cache). */
  body_text: string | null;
}
