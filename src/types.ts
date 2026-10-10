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
