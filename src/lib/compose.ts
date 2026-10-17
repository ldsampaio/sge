import { invoke } from "@tauri-apps/api/core";
import type { ComposeMode, MessageView } from "../types";

/** Original message data needed to seed a Thunderbird-style reply. */
export interface ReplyOriginal {
  from_addr: string;
  to_addrs: string[];
  cc_addrs: string[];
  subject: string;
  date_utc: string;
  body_text: string | null;
  message_id: string | null;
  references: string | null;
}

export interface ReplySeed {
  to: string;
  cc: string;
  subject: string;
  body: string;
  in_reply_to: string | null;
  references: string | null;
}

export function fromMessageView(view: MessageView): ReplyOriginal {
  return {
    from_addr: view.from_addr ?? "",
    to_addrs: view.to_addrs ?? [],
    cc_addrs: view.cc_addrs ?? [],
    subject: view.subject ?? "",
    date_utc: view.date_utc ?? "",
    body_text: view.text ?? null,
    message_id: view.message_id ?? null,
    references: view.references ?? null,
  };
}

/** Split comma/semicolon-separated address lists, trimmed, non-empty. */
export function splitAddrs(list: string): string[] {
  return list
    .split(/[;,]/)
    .map((s) => s.trim())
    .filter((s) => s.length > 0);
}

/**
 * Complete a bare login username into a full sender address using the IMAP
 * host (`mail.utfpr.edu.br` → `user@utfpr.edu.br`) — mirrors the backend
 * `normalize_from_address` as defense in depth (the backend normalizes
 * again at enqueue, so this only fixes what the window displays/sends).
 */
export function buildFromAddress(username: string, imapHost: string | null): string {
  const user = username.trim();
  if (!user || user.includes("@") || !imapHost) return user;
  const host = imapHost.trim().replace(/\.$/, "").toLowerCase();
  if (!host || host === "localhost" || !host.includes(".")) return user;
  const service = ["mail.", "smtp.", "imap.", "email."].find((p) => host.startsWith(p));
  const domain = service ? host.slice(service.length) : host;
  if (!domain || !domain.includes(".")) return user;
  return `${user}@${domain}`;
}

/** Extract the bare email from `Name <addr>` or a bare address, lowercased. */
export function bareAddr(addr: string): string {
  const m = /<([^<>]+)>/.exec(addr);
  return (m ? m[1] : addr).trim().toLowerCase();
}

function dedupeKeepOrder(addrs: string[]): string[] {
  const seen = new Set<string>();
  const out: string[] = [];
  for (const a of addrs) {
    const key = bareAddr(a);
    if (!key || seen.has(key)) continue;
    seen.add(key);
    out.push(a.trim());
  }
  return out;
}

function withoutSelf(addrs: string[], selfAddr: string): string[] {
  const self = bareAddr(selfAddr);
  if (!self) return addrs;
  return addrs.filter((a) => bareAddr(a) !== self);
}

function replySubject(subject: string): string {
  const s = subject.trim();
  if (/^re:/i.test(s)) return s;
  return s ? `Re: ${s}` : "Re: (sem assunto)";
}

function quoteBody(original: ReplyOriginal): string {
  const date = formatQuoteDate(original.date_utc);
  const who = original.from_addr || "(remetente desconhecido)";
  const header = date ? `Em ${date}, ${who} escreveu:` : `${who} escreveu:`;
  const quoted = (original.body_text ?? "")
    .replace(/\r\n/g, "\n")
    .split("\n")
    .map((line) => `> ${line}`)
    .join("\n");
  return `\n\n${header}\n${quoted}`;
}

function formatQuoteDate(date_utc: string): string {
  if (!date_utc) return "";
  const d = new Date(date_utc);
  if (Number.isNaN(d.getTime())) return date_utc;
  return d.toLocaleString("pt-BR");
}

/**
 * Thunderbird-style reply seed.
 *
 * - reply: To = original From; Cc empty; subject `Re:`-prefixed; body quoted.
 * - reply-all: To = original From + original To (minus self); Cc = original
 *   Cc (minus self); same subject/quote threading as reply.
 * - Threading: In-Reply-To = original Message-ID; References = original
 *   References + original Message-ID (RFC 5322 chain).
 */
export function buildReplySeed(
  mode: Exclude<ComposeMode, "new">,
  original: ReplyOriginal,
  selfAddr: string,
): ReplySeed {
  const to =
    mode === "reply"
      ? dedupeKeepOrder([original.from_addr].filter(Boolean))
      : dedupeKeepOrder(
          withoutSelf([original.from_addr, ...original.to_addrs].filter(Boolean), selfAddr),
        );
  const cc =
    mode === "reply-all"
      ? dedupeKeepOrder(withoutSelf(original.cc_addrs.filter(Boolean), selfAddr))
      : [];
  const in_reply_to = original.message_id?.trim() || null;
  const refs = [original.references?.trim(), in_reply_to]
    .filter((s): s is string => !!s && s.length > 0)
    .join(" ");
  return {
    to: to.join(", "),
    cc: cc.join(", "),
    subject: replySubject(original.subject),
    body: quoteBody(original),
    in_reply_to,
    references: refs || null,
  };
}

function isTauriRuntime(): boolean {
  const w = window as unknown as Record<string, unknown>;
  return w["__TAURI_INTERNALS__"] !== undefined || w["__TAURI__"] !== undefined;
}

/** Parse the `#compose?...` hash into mode/uid/mailbox for standalone windows. */
export function parseComposeHash(hash: string): {
  mode: ComposeMode;
  uid: number | null;
  mailbox: string;
} {
  const raw = hash.startsWith("#") ? hash.slice(1) : hash;
  const qIndex = raw.indexOf("?");
  const query = qIndex >= 0 ? raw.slice(qIndex + 1) : "";
  const params = new URLSearchParams(query);
  const modeParam = params.get("mode");
  const mode: ComposeMode = modeParam === "reply" || modeParam === "reply-all" ? modeParam : "new";
  const uidRaw = params.get("uid");
  const uid = uidRaw !== null && /^\d+$/.test(uidRaw) ? Number(uidRaw) : null;
  const mailbox = params.get("mailbox") || "INBOX";
  return { mode, uid, mailbox };
}

export function composeHashFor(mode: ComposeMode, uid: number | null, mailbox: string): string {
  let h = `#compose?mode=${mode}`;
  if (uid !== null) h += `&uid=${uid}`;
  if (mailbox) h += `&mailbox=${encodeURIComponent(mailbox)}`;
  return h;
}

/**
 * Open the Thunderbird-style compose window (standalone OS window in Tauri,
 * new browser tab as fallback). Reply modes carry uid+mailbox so the new
 * window can BODY.PEEK-fetch the original and prefill recipients, subject,
 * quote and threading headers.
 */
export async function openComposeWindow(
  mode: ComposeMode,
  original?: { uid: number; mailbox: string },
): Promise<void> {
  if (isTauriRuntime()) {
    // Explicit nulls: Tauri deserializes Option args from null, never
    // from a missing key on every version.
    await invoke("open_compose_window", {
      mode,
      uid: original?.uid ?? null,
      mailbox: original?.mailbox ?? null,
    });
    return;
  }
  const hash = composeHashFor(mode, original?.uid ?? null, original?.mailbox ?? "INBOX");
  window.open(`${window.location.pathname}${hash}`, "_blank", "width=720,height=600");
}
