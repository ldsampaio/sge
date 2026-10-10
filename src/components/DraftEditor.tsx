import { useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { DiscardResult, DraftRow, DraftSaveResult, FolderTreeResult } from "../types";
import "./MailboxView.css";

/**
 * Autosave cadence (Phase 12 CONTEXT): owned by the open editor like the
 * SyncStatus poll timer — dirty-gated, in-flight-guarded, one interval per
 * open editor, cleared on unmount/editor-switch.
 */
export const DRAFT_AUTOSAVE_INTERVAL_MS = 30_000;

export interface DraftEditorProps {
  /** Compose-session id, or null for a brand-new draft (uuid minted on first save). */
  draftId: string | null;
  /** Seed row from `get_draft` (or a message-row seed); undefined = blank. */
  initial?: DraftRow | null;
  onSaved: (result: DraftSaveResult) => void;
  onDiscarded: (id: string) => void;
  onClose?: () => void;
}

type SavePhase = "idle" | "saving";

interface Snapshot {
  to: string;
  cc: string;
  bcc: string;
  subject: string;
  body: string;
}

function snapshotOf(seed?: DraftRow | null): Snapshot {
  return {
    to: seed?.to ?? "",
    cc: seed?.cc ?? "",
    bcc: seed?.bcc ?? "",
    subject: seed?.subject ?? "",
    body: seed?.body ?? "",
  };
}

function newComposeId(): string {
  try {
    return crypto.randomUUID();
  } catch {
    return `draft-${Date.now().toString(36)}-${Math.floor(Math.random() * 0xffff).toString(36)}`;
  }
}

/** Tauri "no such command" rejections (backend older than the UI). */
function isUnknownCommand(message: string): boolean {
  return /unknown command|no such command|command .* not found|not registered/i.test(message);
}

/**
 * Draft composer (Phase 12, local-first).
 *
 * - Typing marks the form dirty WITHOUT any invoke call (pure snapshot diff).
 * - Save goes through the frozen `save_draft` contract; a `drafts-missing`
 *   refusal offers the Phase 11 create-folder flow behind a confirmation,
 *   then retries exactly once.
 * - Discard confirms only when dirty content exists; pristine drafts close
 *   without a dialog.
 * - The body edits in a plain `<textarea>` — never injected as HTML (T-12-06).
 */
export default function DraftEditor({
  draftId,
  initial,
  onSaved,
  onDiscarded,
  onClose,
}: DraftEditorProps) {
  const seed = useMemo(() => snapshotOf(initial), [initial]);
  const [to, setTo] = useState(seed.to);
  const [cc, setCc] = useState(seed.cc);
  const [bcc, setBcc] = useState(seed.bcc);
  const [subject, setSubject] = useState(seed.subject);
  const [body, setBody] = useState(seed.body);

  /** Last successfully saved field values — the dirty baseline. */
  const savedRef = useRef<Snapshot>(seed);
  /** Stable compose-session id once minted (prop or first-save uuid). */
  const sessionIdRef = useRef<string | null>(draftId);
  /** Rows this editor instance has persisted at least once. */
  const everSavedRef = useRef<boolean>(initial != null);
  /** True while one `save_draft` invoke is in flight (T-12-07: no overlap). */
  const inFlightRef = useRef(false);
  /** `drafts-missing` create-then-retry may run exactly once per save. */
  const retriedRef = useRef(false);

  const [phase, setPhase] = useState<SavePhase>("idle");
  /** Server acknowledgement of the last save (null = never saved yet). */
  const [lastAcked, setLastAcked] = useState<boolean | null>(
    initial != null ? !initial.dirty : null,
  );
  const [errorMsg, setErrorMsg] = useState("");
  const [createPrompt, setCreatePrompt] = useState(false);
  const [createPending, setCreatePending] = useState(false);
  const [discardPrompt, setDiscardPrompt] = useState(false);
  const [discardPending, setDiscardPending] = useState(false);

  const dirty =
    to !== savedRef.current.to ||
    cc !== savedRef.current.cc ||
    bcc !== savedRef.current.bcc ||
    subject !== savedRef.current.subject ||
    body !== savedRef.current.body;
  /** Live mirror so the autosave tick always sees current fields (no stale closure). */
  const liveRef = useRef({ to, cc, bcc, subject, body, dirty });
  liveRef.current = { to, cc, bcc, subject, body, dirty };
  /** MN-06: the mount-once autosave tick captures the first render's
   * `doSave`, so parent callbacks must not close over render state —
   * mirror `onSaved` in a ref and call through it. */
  const onSavedRef = useRef(onSaved);
  onSavedRef.current = onSaved;
  const hasContent =
    dirty ||
    to.trim() !== "" ||
    cc.trim() !== "" ||
    bcc.trim() !== "" ||
    subject.trim() !== "" ||
    body.trim() !== "" ||
    everSavedRef.current;

  async function doSave(): Promise<void> {
    if (inFlightRef.current) return;
    inFlightRef.current = true;
    setPhase("saving");
    setErrorMsg("");
    // Read through the live mirror: manual saves and autosave ticks share
    // this exact path, so a tick can never persist a stale keystroke.
    const { to: liveTo, cc: liveCc, bcc: liveBcc, subject: liveSubject, body: liveBody } =
      liveRef.current;
    try {
      if (sessionIdRef.current === null) {
        sessionIdRef.current = newComposeId();
      }
      const id = sessionIdRef.current;
      const result = await invoke<DraftSaveResult>("save_draft", {
        id,
        subject: liveSubject,
        body: liveBody,
        to: liveTo,
        cc: liveCc,
        bcc: liveBcc,
      });
      savedRef.current = {
        to: liveTo,
        cc: liveCc,
        bcc: liveBcc,
        subject: liveSubject,
        body: liveBody,
      };
      everSavedRef.current = true;
      retriedRef.current = false;
      setLastAcked(result.acked);
      onSavedRef.current(result);
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      if (message.startsWith("drafts-missing") && !retriedRef.current) {
        // Missing Drafts folder — offer the Phase 11 create flow; the retry
        // happens only from the confirm button, exactly once.
        setCreatePrompt(true);
      } else if (isUnknownCommand(message)) {
        setErrorMsg("Recurso de rascunhos indisponível nesta versão do app.");
      } else {
        setErrorMsg(message || "Não foi possível guardar o rascunho.");
      }
    } finally {
      inFlightRef.current = false;
      setPhase("idle");
    }
  }

  /** Confirm branch of the `drafts-missing` flow: create, then retry once. */
  async function handleCreateAndRetry(): Promise<void> {
    if (createPending) return;
    setCreatePending(true);
    setErrorMsg("");
    try {
      await invoke<FolderTreeResult>("create_folder", { leaf: "Drafts", parent: null });
      retriedRef.current = true;
      setCreatePrompt(false);
      await doSave();
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      if (isUnknownCommand(message)) {
        setErrorMsg("Recurso de rascunhos indisponível nesta versão do app.");
      } else {
        setErrorMsg(message || "Não foi possível criar a pasta Rascunhos.");
      }
    } finally {
      setCreatePending(false);
    }
  }

  function handleDiscardRequest(): void {
    // Pristine drafts (nothing typed, never saved) skip the confirm.
    if (!hasContent) {
      handleCloseAfterPristine();
      return;
    }
    setDiscardPrompt(true);
  }

  function handleCloseAfterPristine(): void {
    if (sessionIdRef.current !== null && everSavedRef.current) {
      void confirmDiscard();
    } else {
      // Never persisted — nothing to delete server-side.
      onClose?.();
    }
  }

  async function confirmDiscard(): Promise<void> {
    const id = sessionIdRef.current;
    if (id === null || !everSavedRef.current) {
      setDiscardPrompt(false);
      onClose?.();
      return;
    }
    if (discardPending) return;
    setDiscardPending(true);
    setErrorMsg("");
    try {
      await invoke<DiscardResult>("discard_draft", { id });
      setDiscardPrompt(false);
      onDiscarded(id);
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      if (isUnknownCommand(message)) {
        setErrorMsg("Recurso de rascunhos indisponível nesta versão do app.");
      } else {
        setErrorMsg(message || "Não foi possível descartar o rascunho.");
      }
      setDiscardPrompt(false);
    } finally {
      setDiscardPending(false);
    }
  }

  // Dirty-only autosave tick (Task 3): clean ticks issue zero invokes;
  // dirty ticks run the same save path as the button. Single interval per
  // open editor (remount-per-session via parent key), cleared on unmount.
  // `doSave` reads the live mirror and `onSavedRef`, so the mount-once
  // closure never goes stale (MN-06); the in-flight guard blocks
  // overlapping invokes (T-12-07).
  useEffect(() => {
    const timer = window.setInterval(() => {
      if (liveRef.current.dirty && !inFlightRef.current) {
        void doSave();
      }
    }, DRAFT_AUTOSAVE_INTERVAL_MS);
    return () => {
      window.clearInterval(timer);
    };
    // Intentional: mount-once interval; cleared on unmount.
  }, []);

  // Indicator copy follows the SyncStatus tone (CONTEXT agent's discretion).
  const indicator =
    phase === "saving" ? (
      <p className="sub" role="status">
        Salvando…
      </p>
    ) : dirty ? (
      <p className="sub" role="status">
        Não salvo
      </p>
    ) : lastAcked === false ? (
      <p className="sub" role="status">
        Não salvo — sem conexão
      </p>
    ) : (
      <p className="sub" role="status">
        Salvo
      </p>
    );

  return (
    <aside className="reading-pane" aria-label="Editor de rascunho">
      <form
        role="form"
        aria-label="Editor de rascunho"
        className="draft-editor"
        onSubmit={(e) => {
          e.preventDefault();
          void doSave();
        }}
      >
        <header className="reading-header">
          <span className="reading-kicker">Rascunho</span>
          <h2 className="reading-subject">{subject.trim() || <em>(sem assunto)</em>}</h2>
          {indicator}
        </header>

        <div className="draft-fields">
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
              placeholder="Assunto do rascunho"
              autoComplete="off"
            />
          </label>
          <label>
            Texto
            <textarea
              aria-label="Texto"
              value={body}
              onChange={(e) => setBody(e.target.value)}
              placeholder="Escreva o rascunho aqui…"
              rows={12}
            />
          </label>
        </div>

        {errorMsg && (
          <p className="reading-error" role="alert">
            {errorMsg}
          </p>
        )}

        <div className="reading-actions" role="group" aria-label="Ações do rascunho">
          <button
            type="submit"
            className="btn"
            disabled={phase === "saving" || (!dirty && !everSavedRef.current)}
            aria-label="Guardar rascunho"
            title={!dirty && !everSavedRef.current ? "Escreva algo antes de guardar" : undefined}
          >
            {phase === "saving" ? "Salvando…" : "Guardar"}
          </button>
          <button
            type="button"
            className="btn btn-destructive"
            onClick={handleDiscardRequest}
            disabled={discardPending}
            aria-label="Descartar rascunho"
          >
            Descartar
          </button>
          {onClose && (
            <button type="button" className="btn btn-outline" onClick={onClose} aria-label="Fechar editor">
              Fechar
            </button>
          )}
        </div>
      </form>

      {createPrompt && (
        <div
          className="expunge-modal-backdrop"
          role="alertdialog"
          aria-modal="true"
          aria-labelledby="draft-create-title"
          aria-describedby="draft-create-body"
          onClick={(e) => {
            if (e.target === e.currentTarget) setCreatePrompt(false);
          }}
        >
          <div className="expunge-modal">
            <h2 id="draft-create-title" className="expunge-modal-title">
              Pasta Rascunhos não encontrada
            </h2>
            <p id="draft-create-body" className="expunge-modal-body">
              Pasta Rascunhos não encontrada — criar?
            </p>
            <div className="expunge-modal-actions">
              <button
                type="button"
                className="btn btn-outline"
                onClick={() => setCreatePrompt(false)}
                disabled={createPending}
              >
                Cancelar
              </button>
              <button
                type="button"
                className="btn"
                onClick={() => void handleCreateAndRetry()}
                disabled={createPending}
              >
                {createPending ? "Criando…" : "Criar e guardar"}
              </button>
            </div>
          </div>
        </div>
      )}

      {discardPrompt && (
        <div
          className="expunge-modal-backdrop"
          role="alertdialog"
          aria-modal="true"
          aria-labelledby="draft-discard-title"
          aria-describedby="draft-discard-body"
          onClick={(e) => {
            if (e.target === e.currentTarget) setDiscardPrompt(false);
          }}
        >
          <div className="expunge-modal">
            <h2 id="draft-discard-title" className="expunge-modal-title">
              Descartar rascunho?
            </h2>
            <p id="draft-discard-body" className="expunge-modal-body">
              O conteúdo não salvo será perdido. Não dá para desfazer.
            </p>
            <div className="expunge-modal-actions">
              <button
                type="button"
                className="btn btn-outline"
                onClick={() => setDiscardPrompt(false)}
                disabled={discardPending}
              >
                Continuar editando
              </button>
              <button
                type="button"
                className="btn btn-destructive"
                onClick={() => void confirmDiscard()}
                disabled={discardPending}
              >
                {discardPending ? "Descartando…" : "Descartar"}
              </button>
            </div>
          </div>
        </div>
      )}
    </aside>
  );
}
