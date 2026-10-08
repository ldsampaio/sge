import { useState, useCallback, useEffect, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import Sidebar from "./Sidebar";
import FolderDialog from "./FolderDialog";
import FolderDeleteModal from "./FolderDeleteModal";
import MessageList, { type ListState } from "./MessageList";
import ReadingPane from "./ReadingPane";
import DraftEditor from "./DraftEditor";
import SyncStatus from "./SyncStatus";
import SearchBar from "./SearchBar";
import type {
  MessageRow,
  MessageView,
  DraftRow,
  DraftSaveResult,
  MailboxRow,
  FolderTreeResult,
  RenameFolderResult,
  DeleteFolderResult,
  SyncStatusInfo,
} from "../types";
import { IconCap } from "./icons";
import "./MailboxView.css";

/** True when the raw wire name is the Drafts folder (role first, name fallback). */
function isDraftsFolder(raw: string, mailboxes: MailboxRow[]): boolean {
  const row = mailboxes.find((m) => m.name === raw);
  if (row?.role === "drafts") return true;
  const lower = raw.toLocaleLowerCase();
  return lower === "drafts" || lower === "rascunhos";
}

interface MailboxViewProps {
  mailbox?: string;
}

export default function MailboxView({ mailbox = "INBOX" }: MailboxViewProps) {
  const [selectedMailbox, setSelectedMailbox] = useState(mailbox);
  const [selectedMessage, setSelectedMessage] = useState<MessageRow | null>(null);
  const [searchQuery, setSearchQuery] = useState<string | null>(null);
  const [messageCount, setMessageCount] = useState(0);
  const [mailboxes, setMailboxes] = useState<MailboxRow[]>([]);
  const [listState, setListState] = useState<ListState>({
    kind: "ready",
    message: "",
  });
  const [refreshKey, setRefreshKey] = useState(0);
  // FOLD-04/05/06: folder dialogs (owned here so ops can re-LIST and move
  // selection in the same handler tick — atomic from the user's view).
  const [folderDialog, setFolderDialog] = useState<
    { mode: "create" } | { mode: "rename"; target: string } | null
  >(null);
  const [folderPending, setFolderPending] = useState(false);
  const [folderError, setFolderError] = useState<string | null>(null);
  const [deleteTarget, setDeleteTarget] = useState<{
    wire: string;
    display: string;
    count: number;
  } | null>(null);
  const [deletePending, setDeletePending] = useState(false);
  const [deleteError, setDeleteError] = useState<string | null>(null);
  // DRAFT-01: open draft editor state. Null = reader mode; non-null renders
  // DraftEditor in the reading pane with the compose session (null id = new).
  const [draftEditor, setDraftEditor] = useState<{
    draftId: string | null;
    initial?: DraftRow | null;
  } | null>(null);
  const [draftLoading, setDraftLoading] = useState(false);
  const [draftNotice, setDraftNotice] = useState<string | null>(null);
  /** Server UIDs saved by this session → compose-session ids (row→session map). */
  const sessionDraftsRef = useRef<Map<number, string>>(new Map());

  // FOLD-01: fetch mailbox list from the local store on mount.
  useEffect(() => {
    void (async () => {
      try {
        const rows = await invoke<MailboxRow[]>("list_mailboxes");
        setMailboxes(rows ?? []);
      } catch {
        // No stored mailboxes yet — sidebar falls back to the INBOX bootstrap.
        setMailboxes([]);
      }
    })();
  }, []);

  function handleMessageSelect(msg: MessageRow) {
    // Global search can surface a message from another folder — switch to
    // it so the reader fetches from the right mailbox.
    if (msg.mailbox && msg.mailbox !== selectedMailbox) {
      setSelectedMailbox(msg.mailbox);
      setRefreshKey((k) => k + 1);
    }
    const folder = msg.mailbox || selectedMailbox;
    if (isDraftsFolder(folder, mailboxes)) {
      void openDraftFromRow(msg, folder);
      return;
    }
    setSelectedMessage(msg);
  }

  /**
   * Open a Drafts-folder row in the editor. List rows carry no compose-session
   * id, so: rows saved by this session resolve via the session map straight
   * to `get_draft`; any other row seeds a fresh session from its fetched
   * content (plain-text only — HTML is never injected, T-12-06).
   */
  async function openDraftFromRow(msg: MessageRow, folder: string) {
    setDraftNotice(null);
    setSelectedMessage(null);
    const knownId = sessionDraftsRef.current.get(msg.uid);
    if (knownId !== undefined) {
      setDraftLoading(true);
      try {
        const row = await invoke<DraftRow>("get_draft", { id: knownId });
        setDraftEditor({ draftId: row.id, initial: row });
      } catch (e) {
        setDraftNotice(invokeErrorCopy(e));
      } finally {
        setDraftLoading(false);
      }
      return;
    }
    setDraftLoading(true);
    try {
      const view = await invoke<MessageView>("fetch_message", { uid: msg.uid, mailbox: folder });
      const seed: DraftRow = {
        id: "",
        mailbox_id: 0,
        message_id: "",
        subject: view.subject ?? msg.subject ?? "",
        body: view.text ?? "",
        to: (view.to_addrs ?? []).join(", "),
        cc: "",
        bcc: "",
        dirty: true,
        server_uid: msg.uid,
        attachments: "",
        updated_at: "",
      };
      setDraftEditor({ draftId: null, initial: seed });
    } catch (e) {
      setDraftNotice(invokeErrorCopy(e));
    } finally {
      setDraftLoading(false);
    }
  }

  /** A save lands in the list via the existing refresh path (no full sync). */
  function handleDraftSaved(result: DraftSaveResult) {
    if (result.server_uid !== null && result.server_uid !== undefined) {
      sessionDraftsRef.current.set(result.server_uid, result.id);
    }
    setDraftEditor((ed) => (ed === null ? ed : { ...ed, draftId: result.id }));
    setRefreshKey((k) => k + 1);
  }

  function handleDraftDiscarded(id: string) {
    for (const [uid, known] of sessionDraftsRef.current) {
      if (known === id) sessionDraftsRef.current.delete(uid);
    }
    setDraftEditor(null);
    setRefreshKey((k) => k + 1);
  }

  const handleSearch = useCallback((query: string) => {
    setSearchQuery(query.length > 0 ? query : null);
    setSelectedMessage(null);
  }, []);

  function handleMailboxSelect(m: string) {
    setSelectedMailbox(m);
    setSelectedMessage(null);
    setDraftEditor(null);
    setDraftNotice(null);
    setRefreshKey((k) => k + 1);
  }

  const handleSyncComplete = useCallback(() => {
    setRefreshKey((k) => k + 1);
    // Refresh mailbox list (unread counts may have changed).
    void (async () => {
      try {
        const rows = await invoke<MailboxRow[]>("list_mailboxes");
        setMailboxes(rows ?? []);
      } catch {
        // ignore — stale counts will refresh on next sync
      }
    })();
  }, []);

  /** CREATE round-trip: invoke → re-LIST tree → select the new folder. */
  async function doCreateFolder(wireParent: string, leaf: string) {
    setFolderPending(true);
    setFolderError(null);
    try {
      const res = await invoke<FolderTreeResult>("create_folder", {
        leaf,
        parent: wireParent === "" ? null : wireParent,
      });
      setMailboxes(res.mailboxes ?? []);
      setSelectedMailbox(res.created);
      setSelectedMessage(null);
      setRefreshKey((k) => k + 1);
      setFolderDialog(null);
    } catch (e) {
      // Invoke reject carries the backend UI-SPEC copy — inline, no toast.
      setFolderError(invokeErrorCopy(e));
    } finally {
      setFolderPending(false);
    }
  }

  /** RENAME round-trip: invoke → migrate selection to the new name in the
   * same tick as the tree refresh (CONTEXT locked, no stale selection). */
  async function doRenameFolder(target: string, leaf: string) {
    setFolderPending(true);
    setFolderError(null);
    try {
      const res = await invoke<RenameFolderResult>("rename_folder", {
        old: target,
        new_leaf: leaf,
      });
      setMailboxes(res.mailboxes ?? []);
      setSelectedMailbox(res.new);
      setSelectedMessage(null);
      setRefreshKey((k) => k + 1);
      setFolderDialog(null);
      if (res.warning) console.warn(`[SGE folders] rename warning: ${res.warning}`);
    } catch (e) {
      setFolderError(invokeErrorCopy(e));
    } finally {
      setFolderPending(false);
    }
  }

  function handleFolderConfirm(wireParent: string, leaf: string) {
    if (folderDialog?.mode === "rename") {
      void doRenameFolder(folderDialog.target, leaf);
    } else {
      void doCreateFolder(wireParent, leaf);
    }
  }

  /** DELETE step 1: probe the local count to pick the modal variant, then
   * show the confirm modal. The backend re-checks freshly — it is the
   * source of truth (a raced `need_delete_confirm` bumps the count). */
  async function handleDeleteRequest(wire: string) {
    const row = mailboxes.find((m) => m.name === wire);
    const display = row?.display_name ?? wire;
    let count: number;
    try {
      const s = await invoke<SyncStatusInfo>("sync_status", { mailbox: wire });
      count = s.message_count ?? 0;
    } catch {
      count = 0;
    }
    setDeleteError(null);
    setDeleteTarget({ wire, display, count });
  }

  /** DELETE step 2: confirmed from the modal (typed name for non-empty). */
  async function handleDeleteConfirm(typedName: string | null) {
    if (!deleteTarget) return;
    setDeletePending(true);
    setDeleteError(null);
    try {
      const res = await invoke<DeleteFolderResult>("delete_folder", {
        name: deleteTarget.wire,
        confirmed: deleteTarget.count > 0,
        typed_name: typedName,
      });
      setMailboxes(res.mailboxes ?? []);
      setSelectedMailbox(res.fallback);
      setSelectedMessage(null);
      setRefreshKey((k) => k + 1);
      setDeleteTarget(null);
    } catch (e) {
      const msg = invokeErrorCopy(e);
      // Fresh-count race: the folder filled between probe and confirm —
      // switch the modal to the non-empty variant instead of failing.
      const needCount = /^need_delete_confirm:(\d+)$/.exec(msg);
      if (needCount && deleteTarget) {
        setDeleteTarget({ ...deleteTarget, count: Number(needCount[1]) });
      } else {
        // Typed-name race (folder renamed between probe and confirm):
        // surface the backend copy, never the raw wire protocol string.
        const needTyped = /^need_typed_confirm:(.+)$/.exec(msg);
        setDeleteError(
          needTyped
            ? `O nome da pasta mudou — digite ${needTyped[1]} para confirmar.`
            : msg,
        );
      }
    } finally {
      setDeletePending(false);
    }
  }

  function invokeErrorCopy(e: unknown): string {
    return typeof e === "string"
      ? e
      : e instanceof Error
        ? e.message
        : "Não foi possível concluir a operação.";
  }

  // Rename dialog prefill: decoded leaf segment of the target row.
  const renameTarget = folderDialog?.mode === "rename" ? folderDialog.target : null;
  const renameRow =
    renameTarget !== null ? mailboxes.find((m) => m.name === renameTarget) : undefined;
  const renameDelim = renameRow?.delimiter ?? "";
  const renameLeaf =
    renameRow !== undefined && renameDelim !== "" && renameRow.display_name.includes(renameDelim)
      ? (renameRow.display_name.split(renameDelim).pop() ?? renameRow.display_name)
      : (renameRow?.display_name ?? "");

  const showingDrafts = isDraftsFolder(selectedMailbox, mailboxes);

  return (
    <div className="mailbox-layout">
      <header className="mailbox-header">
        <div className="mailbox-titleblock">
          <span className="mailbox-avatar" aria-hidden="true">
            <IconCap size={24} />
          </span>
          <div>
            <h2>Sala de aula · Caixa de entrada</h2>
            <p className="mailbox-subtitle">
              Avisos e materiais da escola, organizados para estudar offline
            </p>
          </div>
        </div>
        <SyncStatus
          key={selectedMailbox}
          mailbox={selectedMailbox}
          mailboxes={mailboxes}
          onSyncComplete={handleSyncComplete}
        />
      </header>
      <div className="mailbox-panes">
        <Sidebar
          selectedMailbox={selectedMailbox}
          mailboxes={mailboxes}
          onMailboxSelect={handleMailboxSelect}
          onCreateFolder={() => {
            setFolderError(null);
            setFolderDialog({ mode: "create" });
          }}
          onRenameFolder={(m) => {
            setFolderError(null);
            setFolderDialog({ mode: "rename", target: m });
          }}
          onDeleteFolder={(m) => {
            void handleDeleteRequest(m);
          }}
        />
        <main className="message-panel" aria-label="Fila de leitura">
          <div className="message-list-header">
            <SearchBar onSearch={handleSearch} disabled={listState.kind === "loading"} />
            {listState.kind === "ready" && (
              <span className="message-count" aria-live="polite">
                {messageCount} {messageCount === 1 ? "mensagem" : "mensagens"}
              </span>
            )}
          </div>
          {showingDrafts && (
            <button
              type="button"
              className="btn draft-new-btn"
              onClick={() => {
                setSelectedMessage(null);
                setDraftNotice(null);
                setDraftEditor({ draftId: null });
              }}
              aria-label="Novo rascunho"
            >
              Novo rascunho
            </button>
          )}
          {draftNotice && (
            <p className="reading-error" role="alert">
              {draftNotice}
            </p>
          )}
          <MessageList
            key={selectedMailbox}
            mailbox={selectedMailbox}
            mailboxes={mailboxes}
            searchQuery={searchQuery}
            refreshKey={refreshKey}
            onMessageSelect={handleMessageSelect}
            selectedUid={selectedMessage?.uid ?? null}
            onStateChange={setListState}
            onMessageCount={setMessageCount}
          />
        </main>
        {draftEditor !== null || draftLoading ? (
          <DraftEditorPane
            draftEditor={draftEditor}
            loading={draftLoading}
            onSaved={handleDraftSaved}
            onDiscarded={handleDraftDiscarded}
            onClose={() => setDraftEditor(null)}
          />
        ) : (
          <ReadingPane
            key={selectedMailbox}
            selectedMessage={selectedMessage}
            mailbox={selectedMailbox}
            mailboxes={mailboxes}
          />
        )}
      </div>
      {(() => {
        const isRename = renameTarget !== null;
        return (
          <FolderDialog
            mode={isRename ? "rename" : "create"}
            delimiter={
              isRename
                ? renameDelim
                : (mailboxes.find((m) => m.name === selectedMailbox)?.delimiter ?? "")
            }
            parents={mailboxes}
            existingNames={mailboxes.map((m) => m.name)}
            onConfirm={handleFolderConfirm}
            onCancel={() => setFolderDialog(null)}
            isOpen={folderDialog !== null}
            serverError={folderError}
            pending={folderPending}
            renameDisplay={isRename ? (renameRow?.display_name ?? "") : undefined}
            initialName={isRename ? renameLeaf : undefined}
          />
        );
      })()}
      <FolderDeleteModal
        displayName={deleteTarget?.display ?? ""}
        count={deleteTarget?.count ?? 0}
        serverError={deleteError}
        pending={deletePending}
        onConfirm={(typed) => {
          void handleDeleteConfirm(typed);
        }}
        onCancel={() => setDeleteTarget(null)}
        isOpen={deleteTarget !== null}
      />
    </div>
  );
}

/**
 * Right-pane slot for draft editing: a loading placeholder while the seed
 * loads, then the editor keyed by compose session so switching drafts
 * remounts cleanly.
 */
function DraftEditorPane({
  draftEditor,
  loading,
  onSaved,
  onDiscarded,
  onClose,
}: {
  draftEditor: { draftId: string | null; initial?: DraftRow | null } | null;
  loading: boolean;
  onSaved: (result: DraftSaveResult) => void;
  onDiscarded: (id: string) => void;
  onClose: () => void;
}) {
  if (draftEditor === null) {
    return (
      <aside className="reading-pane flex-center" aria-label="Editor de rascunho">
        <p className="reading-placeholder" role="status">
          {loading ? "Abrindo o rascunho…" : "Escolha um rascunho para editar"}
        </p>
      </aside>
    );
  }
  return (
    <DraftEditor
      key={draftEditor.draftId ?? "new"}
      draftId={draftEditor.draftId}
      initial={draftEditor.initial}
      onSaved={onSaved}
      onDiscarded={onDiscarded}
      onClose={onClose}
    />
  );
}
