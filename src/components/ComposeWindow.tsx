// Compose Window (Thunderbird-style) — standalone email editor.
//
// Opened as a separate OS window (Tauri `open_compose_window`) by the
// New Message / Reply / Reply All buttons. Reply windows prefill
// recipients/subject/quote/threading from the answered message
// (BODY.PEEK fetch — never sets \Seen). The Send button enqueues via
// `queue_send` and flushes via `send_pending` (SMTP + Sent APPEND); the
// copy in Sent is filed by the backend on success.
//
// Attachments: drag-and-drop files onto the body field (or the Anexar
// button) stages them as base64; they ride `queue_send` and are rendered
// multipart/mixed by the backend (25 MB cap enforced there).

import { useCallback, useEffect, useRef, useState } from "react";
import { invoke, Channel } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import {
  buildFromAddress,
  buildReplySeed,
  composeHashFor,
  fromMessageView,
  parseComposeHash,
  splitAddrs,
} from "../lib/compose";
import type {
  ComposeMode,
  MessageView,
  OutgoingAttachmentPayload,
  QueueSendResult,
  SendPendingResult,
  SendRowState,
} from "../types";
import "./ComposeWindow.css";

export interface ComposePrefill {
  to: string;
  cc: string;
  bcc: string;
  subject: string;
  body: string;
  in_reply_to: string | null;
  references: string | null;
}

export interface ComposeWindowProps {
  /** Standalone mode: "new" (blank) or reply with a fetch source. */
  mode?: ComposeMode;
  /** Reply source — the window BODY.PEEK-fetches the original for prefill. */
  replySource?: { uid: number; mailbox: string } | null;
  /** Ready-made prefill (takes precedence over `replySource`). */
  initial?: ComposePrefill | null;
  /** Called after a successful send (embedded use). */
  onSent?: (queueId: string) => void;
  /** Called when the window should close (embedded use). */
  onClose?: () => void;
}

interface SavedCredentials {
  username: string;
}

interface ServerConfig {
  host: string;
  port: number;
  security: string;
}

/** Client-side pre-check on the *decoded* attachment size. The backend cap
 *  is 25 MB on the rendered message, and base64 inflates by 4/3 — so ~18 MB
 *  of files is the largest payload that still fits under the wire limit.
 *  The backend remains the authority (`send-too-large`). */
const MAX_TOTAL_ATTACH_BYTES = 18 * 1024 * 1024;

/** True when a `generate_reply` rejection is the user's own cancel. */
function isCancelledError(message: string): boolean {
  return /cancel/i.test(message);
}

function isTauriRuntime(): boolean {
  const w = window as unknown as Record<string, unknown>;
  return w["__TAURI_INTERNALS__"] !== undefined || w["__TAURI__"] !== undefined;
}

/** Bound for one `send_pending` round-trip (SMTP up to 60 s/cmd + bounded
 *  IMAP filing): the backend always answers inside this window now, but the
 *  UI must never stick on "Enviando…" forever if a backend ever hangs. */
const SEND_PENDING_TIMEOUT_MS = 150_000;

/** Race an invoke against a timeout so the compose window always settles. */
function withTimeout<T>(promise: Promise<T>, ms: number, label: string): Promise<T> {
  let timer: ReturnType<typeof setTimeout> | null = null;
  const timeout = new Promise<never>((_, reject) => {
    timer = setTimeout(() => reject(new Error(label)), ms);
  });
  return Promise.race([
    promise.finally(() => {
      if (timer !== null) clearTimeout(timer);
    }),
    timeout,
  ]);
}

function sendPendingNow(): Promise<SendPendingResult> {
  return withTimeout(
    invoke<SendPendingResult>("send_pending", {}),
    SEND_PENDING_TIMEOUT_MS,
    "o envio está demorando além do limite (150 s) — a mensagem segue na fila e o app continua tentando; verifique Enviadas antes de reenviar",
  );
}

/** Best-effort Sent re-sync so the filed copy shows up immediately
 *  instead of hiding behind the stale local cache. Fire-and-forget: the
 *  backend pass keeps running even if this window closes first. */
function refreshSentFolder(sentWire: string | null): void {
  const mailbox = sentWire || "Sent";
  void (async () => {
    try {
      const channel = new Channel<unknown>();
      await invoke("start_sync", { onEvent: channel, mailbox });
    } catch {
      // Best-effort: the next poll covers it.
    }
  })();
}

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

function estimatePayloadBytes(att: OutgoingAttachmentPayload): number {
  const b64 = att.data_base64.includes("base64,")
    ? att.data_base64.split("base64,")[1]
    : att.data_base64;
  return Math.floor(b64.length * 0.75);
}

function readFileAsDataURL(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => resolve(String(reader.result ?? ""));
    reader.onerror = () => reject(new Error(`Não foi possível ler ${file.name}`));
    reader.readAsDataURL(file);
  });
}

export function ComposeWindowFromHash(): React.JSX.Element {
  const parsed = parseComposeHash(window.location.hash);
  const replySource =
    parsed.mode !== "new" && parsed.uid !== null
      ? { uid: parsed.uid, mailbox: parsed.mailbox }
      : null;
  return (
    <div className="compose-standalone">
      <ComposeWindow mode={parsed.mode} replySource={replySource} standalone />
    </div>
  );
}

export default function ComposeWindow({
  mode = "new",
  replySource = null,
  initial = null,
  onSent,
  onClose,
  standalone = false,
}: ComposeWindowProps & { standalone?: boolean }): React.JSX.Element {
  const [to, setTo] = useState(initial?.to ?? "");
  const [cc, setCc] = useState(initial?.cc ?? "");
  const [bcc, setBcc] = useState(initial?.bcc ?? "");
  const [subject, setSubject] = useState(initial?.subject ?? "");
  const [body, setBody] = useState(initial?.body ?? "");
  const [inReplyTo, setInReplyTo] = useState<string | null>(initial?.in_reply_to ?? null);
  const [references, setReferences] = useState<string | null>(initial?.references ?? null);
  const [selfAddr, setSelfAddr] = useState("");
  const [attachments, setAttachments] = useState<OutgoingAttachmentPayload[]>([]);
  const [loading, setLoading] = useState(
    mode !== "new" && initial === null && replySource !== null,
  );
  const [phase, setPhase] = useState<"idle" | "sending" | "filing">("idle");
  const [llmGeneration, setLlmGeneration] = useState<"idle" | "generating" | "error">("idle");
  const [llmError, setLlmError] = useState<string | null>(null);
  const [generateRequested, setGenerateRequested] = useState(false);
  const [unfiled, setUnfiled] = useState(false);
  const [notice, setNotice] = useState<{ kind: "error" | "ok"; text: string } | null>(null);
  const [dragActive, setDragActive] = useState(false);
  const fileInputRef = useRef<HTMLInputElement | null>(null);
  const seededRef = useRef(initial !== null || mode === "new");

  // Self address (From/envelope) from the remembered credentials,
  // completed with the server domain when the login is a bare username.
  useEffect(() => {
    void (async () => {
      try {
        const creds = await invoke<SavedCredentials | null>("load_credentials");
        if (!creds?.username) return;
        let host: string | null = null;
        try {
          const cfg = await invoke<ServerConfig | null>("load_server_config");
          host = cfg?.host ?? null;
        } catch {
          host = null;
        }
        setSelfAddr(buildFromAddress(creds.username, host));
      } catch {
        // No remembered credentials — send will ask to log in first.
      }
    })();
  }, []);

  // Reply prefill: BODY.PEEK-fetch the original, seed Thunderbird-style.
  useEffect(() => {
    if (seededRef.current || mode === "new" || !replySource) return;
    let cancelled = false;
    void (async () => {
      setLoading(true);
      setNotice(null);
      try {
        const view = await invoke<MessageView>("fetch_message", {
          uid: replySource.uid,
          mailbox: replySource.mailbox,
        });
        if (cancelled) return;
        let self = selfAddr;
        if (!self) {
          try {
            const creds = await invoke<SavedCredentials | null>("load_credentials");
            let host: string | null = null;
            try {
              const cfg = await invoke<ServerConfig | null>("load_server_config");
              host = cfg?.host ?? null;
            } catch {
              host = null;
            }
            self = buildFromAddress(creds?.username ?? "", host);
            if (!cancelled) setSelfAddr(self);
          } catch {
            self = "";
          }
        }
        const seed = buildReplySeed(mode, fromMessageView(view), self);
        if (cancelled) return;
        setTo(seed.to);
        setCc(seed.cc);
        setSubject(seed.subject);
        setBody(seed.body);
        setInReplyTo(seed.in_reply_to);
        setReferences(seed.references);
        seededRef.current = true;
      } catch (e) {
        if (!cancelled) {
          setNotice({
            kind: "error",
            text: `Não foi possível carregar a mensagem original: ${String(e)}`,
          });
        }
      } finally {
        if (!cancelled) setLoading(false);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  const addFiles = useCallback(async (files: FileList | File[]) => {
    const list = Array.from(files);
    if (list.length === 0) return;
    setNotice(null);
    try {
      const staged: OutgoingAttachmentPayload[] = await Promise.all(
        list.map(async (file) => ({
          filename: file.name,
          content_type: file.type || "application/octet-stream",
          data_base64: await readFileAsDataURL(file),
        })),
      );
      setAttachments((prev) => {
        const next = [...prev, ...staged];
        const total = next.reduce((acc, a) => acc + estimatePayloadBytes(a), 0);
        if (total > MAX_TOTAL_ATTACH_BYTES) {
          setNotice({
            kind: "error",
            text: `Anexos somam ${formatBytes(total)} — o limite de envio é 25 MB (cerca de 18 MB de arquivos, por causa da codificação). Remova arquivos para continuar.`,
          });
        }
        return next;
      });
    } catch (e) {
      setNotice({ kind: "error", text: String(e) });
    }
  }, []);

  function removeAttachment(index: number): void {
    setAttachments((prev) => prev.filter((_, i) => i !== index));
    setNotice(null);
  }

  async function close(): Promise<void> {
    if (onClose) {
      onClose();
      return;
    }
    if (standalone && isTauriRuntime()) {
      // Most reliable first: backend closes the calling window directly
      // (no IPC capability involved, unlike the frontend close paths).
      try {
        await invoke("close_compose_window", {});
        return;
      } catch {
        // Fall through to the frontend close paths below.
      }
      try {
        await getCurrentWindow().close();
        return;
      } catch {
        // Fall through to window.close() below.
      }
    }
    window.close();
  }

  /** This window's queue row (set once Enviar enqueues — never re-enqueued
   *  afterwards, so a filing retry cannot duplicate the message). */
  const lastQueueIdRef = useRef<string | null>(null);

  /** Per-message close decision: aggregate `send_status()` counts mix every
   *  row together, so a stranded row from yesterday could hold today's
   *  window open. The window asks about ITS `queue_id` only. Returns true
   *  when the window is closing (filed) — false keeps it open with the
   *  reason + filing retry. */
  async function settleByRow(queueId: string, summary: SendPendingResult): Promise<boolean> {
    let row: SendRowState | null;
    try {
      row = await invoke<SendRowState>("send_row_state", { queue_id: queueId });
    } catch {
      row = null;
    }
    const state = row?.state;
    const reason = row?.last_error ?? summary.last_error;
    if (state === "sent") {
      setUnfiled(false);
      setNotice({ kind: "ok", text: "Mensagem enviada e arquivada em Enviadas." });
      onSent?.(queueId);
      refreshSentFolder(summary.sent_wire);
      // Thunderbird-like: close the compose window after a successful send.
      window.setTimeout(() => void close(), 600);
      return true;
    }
    if (state === "sent_unfiled") {
      // Delivered but the Sent copy is still missing: keep the window
      // open with a filing retry (auto-close would pretend it is filed).
      setUnfiled(true);
      setNotice({
        kind: "error",
        text:
          reason ??
          "Mensagem entregue, mas a cópia em Enviadas ainda não foi arquivada. Use “Tentar arquivar” ou feche — o app tenta de novo no próximo envio.",
      });
      onSent?.(queueId);
      return false;
    }
    if (state === "uncertain") {
      setNotice({
        kind: "error",
        text:
          reason ??
          "O servidor pode já ter recebido a mensagem — ela será conferida em Enviadas antes de qualquer reenvio.",
      });
      return false;
    }
    if (state === "failed") {
      setNotice({
        kind: "error",
        text: reason ?? "Não foi possível enviar — a mensagem ficou na fila com falha.",
      });
      return false;
    }
    // queued/sending/unknown: fall back to the pass summary (the backend
    // may still be settling the row).
    if (summary.sent > 0 && summary.sent_unfiled === 0) {
      setUnfiled(false);
      setNotice({ kind: "ok", text: "Mensagem enviada e arquivada em Enviadas." });
      onSent?.(queueId);
      refreshSentFolder(summary.sent_wire);
      window.setTimeout(() => void close(), 600);
      return true;
    }
    setNotice({
      kind: "error",
      text: reason ?? "Não foi possível enviar agora — a mensagem ficou na fila e será retentada.",
    });
    return false;
  }

  async function handleSend(): Promise<void> {
    if (phase === "sending") return;
    const toList = splitAddrs(to);
    const ccList = splitAddrs(cc);
    const bccList = splitAddrs(bcc);
    if (toList.length + ccList.length + bccList.length === 0) {
      setNotice({ kind: "error", text: "Adicione ao menos um destinatário antes de enviar." });
      return;
    }
    if (!selfAddr) {
      setNotice({ kind: "error", text: "Entre na sua conta na janela principal antes de enviar." });
      return;
    }
    setPhase("sending");
    setNotice(null);
    try {
      const queued = await invoke<QueueSendResult>("queue_send", {
        from: selfAddr,
        to: toList,
        cc: ccList,
        bcc: bccList,
        subject,
        body,
        draft_id: null,
        in_reply_to: inReplyTo,
        references,
        attachments,
      });
      let summary: SendPendingResult;
      try {
        summary = await sendPendingNow();
      } catch (e) {
        // Enqueued durably but the flush could not run (offline?): the
        // message stays queued and a later sync/send retries it.
        setNotice({
          kind: "error",
          text: `Na fila para envio (${queued.queue_id}); o envio imediato falhou: ${String(e)} — será retentado.`,
        });
        return;
      }
      lastQueueIdRef.current = queued.queue_id;
      await settleByRow(queued.queue_id, summary);
    } catch (e) {
      setNotice({ kind: "error", text: `Falha ao enviar: ${String(e)}` });
    } finally {
      setPhase("idle");
    }
  }

  /** Filing-only retry: flushes + APPEND-retries pending copies WITHOUT
   *  re-enqueueing (calling Enviar again would duplicate the message). */
  async function handleFileRetry(): Promise<void> {
    if (phase !== "idle") return;
    setPhase("filing");
    setNotice(null);
    try {
      const summary = await sendPendingNow();
      const queueId = lastQueueIdRef.current;
      if (queueId === null) {
        // No row from this window (e.g. reloaded mid-retry): decide by counts.
        if (summary.sent_unfiled === 0) {
          setUnfiled(false);
          setNotice({ kind: "ok", text: "Cópia arquivada em Enviadas." });
          refreshSentFolder(summary.sent_wire);
          window.setTimeout(() => void close(), 600);
        } else {
          setNotice({
            kind: "error",
            text:
              summary.last_error ??
              "Ainda não foi possível arquivar em Enviadas — o app continua tentando.",
          });
        }
        return;
      }
      await settleByRow(queueId, summary);
    } catch (e) {
      setNotice({ kind: "error", text: `Falha ao arquivar: ${String(e)}` });
    } finally {
      setPhase("idle");
    }
  }

  /** Generate reply draft with LLM and insert above existing body.
   *  LLM-08: Insert above quoted original, fully editable.
   *  LLM-10: Regenerate without losing compose content; failed generation never clobbers body.
   */
  async function handleGenerateWithLLM(): Promise<void> {
    if (llmGeneration === "generating") return;
    if (!replySource) return;

    setGenerateRequested(true);
    setLlmGeneration("generating");
    setLlmError(null);
    setNotice(null);

    try {
      // The backend rebuilds the prompt from the *original* message in the
      // local BODY.PEEK cache (sender/subject/body) and only falls back to
      // these compose-state values — so replies are never generated from
      // the wrong message anymore.
      const draft = await invoke<string>("generate_reply", {
        mailbox: replySource.mailbox,
        uid: replySource.uid,
        request_id: null,
        subject,
        body,
      });

      // Insert draft ABOVE existing body, never replacing it (LLM-10: never clobber)
      const currentBody = body || "";
      const separator = currentBody ? "\n\n---\n\n" : "";
      setBody(draft + separator + currentBody);

      setLlmGeneration("idle");
      setNotice({ kind: "ok", text: "Rascunho gerado — revise e edite antes de enviar." });
    } catch (e: unknown) {
      const errMsg = e instanceof Error ? e.message : String(e);
      if (isCancelledError(errMsg)) {
        // User pressed Cancelar: never an error state, never a body change.
        setLlmGeneration("idle");
        return;
      }
      setLlmGeneration("error");
      setLlmError(errMsg);
      // LLM-10: Failed generation never clobbers the body — do NOT modify body state
      setNotice({ kind: "error", text: `Falha ao gerar: ${errMsg}` });
    } finally {
      setGenerateRequested(false);
    }
  }

  async function handleCancelGenerate(): Promise<void> {
    // Real cancel: clears the shared tracker slot in the backend so an
    // in-flight request is discarded instead of being inserted into the body.
    try {
      await invoke<boolean>("cancel_generate", { mailbox: replySource?.mailbox ?? "INBOX" });
    } catch {
      // Best-effort: the generation result is still dropped locally below.
    }
    setLlmGeneration("idle");
    setGenerateRequested(false);
    setNotice({ kind: "ok", text: "Geração cancelada." });
  }

  const title =
    mode === "reply" ? "Responder" : mode === "reply-all" ? "Responder a todos" : "Nova Mensagem";
  const totalAttach = attachments.reduce((acc, a) => acc + estimatePayloadBytes(a), 0);

  if (loading) {
    return (
      <div className="compose-window" role="status" aria-live="polite">
        <h2>{title}</h2>
        <p className="sub">Carregando mensagem original…</p>
      </div>
    );
  }

  return (
    <div className="compose-window">
      <h2>{title}</h2>
      {selfAddr && (
        <p className="sub">
          De: {selfAddr}
          {inReplyTo && <> · em resposta (threading RFC 5322)</>}
        </p>
      )}
      <div className="compose-fields">
        <label>
          Para
          <input
            type="text"
            aria-label="Para"
            value={to}
            onChange={(e) => setTo(e.target.value)}
            placeholder="destinatario@exemplo.com, outro@exemplo.com"
            autoComplete="off"
          />
        </label>
        <label>
          Cc
          <input
            type="text"
            aria-label="Cc"
            value={cc}
            onChange={(e) => setCc(e.target.value)}
            placeholder="copia@exemplo.com"
            autoComplete="off"
          />
        </label>
        <label>
          Cco
          <input
            type="text"
            aria-label="Cco"
            value={bcc}
            onChange={(e) => setBcc(e.target.value)}
            placeholder="copia-oculta@exemplo.com"
            autoComplete="off"
          />
        </label>
        <label>
          Assunto
          <input
            type="text"
            aria-label="Assunto"
            value={subject}
            onChange={(e) => setSubject(e.target.value)}
            placeholder="Assunto da mensagem"
            autoComplete="off"
          />
        </label>
        <label>
          Texto
          <div
            className={
              dragActive ? "compose-dropzone compose-dropzone--active" : "compose-dropzone"
            }
            onDragOver={(e) => {
              e.preventDefault();
              setDragActive(true);
            }}
            onDragLeave={() => setDragActive(false)}
            onDrop={(e) => {
              e.preventDefault();
              setDragActive(false);
              if (e.dataTransfer?.files?.length) void addFiles(e.dataTransfer.files);
            }}
          >
            <textarea
              aria-label="Texto"
              value={body}
              onChange={(e) => setBody(e.target.value)}
              placeholder="Escreva a mensagem aqui… Arraste arquivos para cá para anexar."
              rows={12}
            />
            {dragActive && <p className="compose-drop-hint">Solte para anexar</p>}
          </div>
        </label>
      </div>

      <div className="compose-attachments" aria-live="polite">
        <div className="compose-attachments-head">
          <span>
            Anexos · {attachments.length}
            {totalAttach > 0 && ` · ${formatBytes(totalAttach)}`}
          </span>
          <button
            type="button"
            className="btn btn-outline btn-small"
            onClick={() => fileInputRef.current?.click()}
          >
            Anexar arquivos
          </button>
          <input
            ref={fileInputRef}
            type="file"
            multiple
            hidden
            aria-label="Anexar arquivos"
            onChange={(e) => {
              if (e.target.files?.length) void addFiles(e.target.files);
              e.target.value = "";
            }}
          />
        </div>
        {attachments.length > 0 && (
          <ul className="compose-attachment-list">
            {attachments.map((att, i) => (
              <li key={`${att.filename}-${i}`}>
                <span title={att.content_type}>
                  {att.filename} · {formatBytes(estimatePayloadBytes(att))}
                </span>
                <button
                  type="button"
                  className="btn btn-ghost btn-small"
                  aria-label={`Remover anexo ${att.filename}`}
                  onClick={() => removeAttachment(i)}
                >
                  Remover
                </button>
              </li>
            ))}
          </ul>
        )}
      </div>

      {notice && (
        <p
          className={notice.kind === "error" ? "reading-error" : "compose-ok"}
          role={notice.kind === "error" ? "alert" : "status"}
        >
          {notice.text}
        </p>
      )}

      <div className="compose-actions" role="group" aria-label="Ações da mensagem">
        <button
          type="button"
          className="btn"
          onClick={() => void handleSend()}
          disabled={phase !== "idle"}
          aria-label="Enviar"
        >
          {phase === "sending" ? "Enviando…" : "Enviar"}
        </button>
        {unfiled && (
          <button
            type="button"
            className="btn btn-outline"
            onClick={() => void handleFileRetry()}
            disabled={phase !== "idle"}
            aria-label="Tentar arquivar em Enviadas"
            title="Reenvia apenas o arquivamento em Enviadas — a mensagem não é duplicada"
          >
            {phase === "filing" ? "Arquivando…" : "Tentar arquivar"}
          </button>
        )}
        <button
          type="button"
          className="btn btn-outline"
          onClick={() => void close()}
          aria-label="Fechar"
        >
          Fechar
        </button>
        {mode !== "new" && replySource && (
          <>
            <button
              type="button"
              className="btn"
              onClick={() => void handleGenerateWithLLM()}
              disabled={llmGeneration === "generating" || phase !== "idle"}
              aria-label="Gerar resposta com LLM"
            >
              {llmGeneration === "generating" ? "Gerando…" : "Gerar com LLM"}
            </button>
            {generateRequested && (
              <button
                type="button"
                className="btn btn-outline"
                onClick={() => void handleCancelGenerate()}
                disabled={llmGeneration !== "generating"}
                aria-label="Cancelar geração"
              >
                Cancelar
              </button>
            )}
            {llmError && (
              <p className="reading-error" role="alert">
                {llmError}
              </p>
            )}
          </>
        )}
      </div>
      {/* Keep the hash in sync for standalone windows (reload keeps context). */}
      <ComposeHashSync mode={mode} replySource={replySource} />
    </div>
  );
}

function ComposeHashSync({
  mode,
  replySource,
}: {
  mode: ComposeMode;
  replySource: { uid: number; mailbox: string } | null;
}): React.JSX.Element | null {
  useEffect(() => {
    if (!window.location.hash.startsWith("#compose")) return;
    const want = composeHashFor(mode, replySource?.uid ?? null, replySource?.mailbox ?? "INBOX");
    if (window.location.hash !== want) window.location.hash = want;
  }, [mode, replySource]);
  return null;
}
