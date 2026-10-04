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
