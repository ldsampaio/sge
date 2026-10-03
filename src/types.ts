// Shared types for the mailbox UI (Phase 3).
// Mirrors the Rust MessageRow struct from store/queries.rs.

export interface MessageRow {
  uid: number;
  subject: string;
  from_addr: string;
  to_addrs: string;
  date_utc: string;
  flags: string; // JSON array string, e.g. ["\\Seen"] or []
  has_attachments: boolean;
  preview: string;
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
 * Humanize a date_utc ISO string into a short relative display.
 * e.g. "Today 10:30 AM", "Yesterday", "Oct 1".
 */
export function humanizeDate(date_utc: string): string {
  try {
    const date = new Date(date_utc);
    const now = new Date();
    const diffMs = now.getTime() - date.getTime();
    const diffDays = Math.floor(diffMs / (1000 * 60 * 60 * 24));

    const timeStr = date.toLocaleTimeString([], {
      hour: "2-digit",
      minute: "2-digit",
    });

    if (diffDays === 0) return `Today ${timeStr}`;
    if (diffDays === 1) return `Yesterday`;
    if (diffDays < 7) {
      const dayName = date.toLocaleDateString([], { weekday: "short" });
      return dayName;
    }
    // Within current year → month + day
    if (date.getFullYear() === now.getFullYear()) {
      return date.toLocaleDateString([], { month: "short", day: "numeric" });
    }
    return date.toLocaleDateString([], {
      year: "numeric",
      month: "short",
      day: "numeric",
    });
  } catch {
    return date_utc;
  }
}
