# Phase 10: Delete + Move - Context

**Gathered:** 2026-10-06
**Status:** Ready for planning
**Mode:** Smart discuss (autonomous)

<domain>
## Phase Boundary

User can delete messages (Trash by default, expunge when explicit) and move them between folders, trusting it survives offline and never nukes other clients' mail. Covers DEL-01, DEL-02, MOVE-01. Depends on Phase 9 (v1.1 complete).

</domain>

<decisions>
## Implementation Decisions

### Delete semantics
- Apagar vai para Trash + desfazer (padrão Thunderbird/Gmail)
- Se não houver pasta Trash, criar 'Trash' via CREATE na hora
- Undo vale até o próximo sync (outbox pendente como âncora)

### Expunge
- Confirmação em modal "Apagar para sempre? X mensagens" com nome da pasta
- Só UIDs selecionados (UID EXPUNGE, nunca expunge cego na pasta)
- Nunca tocar mensagens marcadas por outros clients (só UIDs próprios)

### Move
- UI de destino: menu 'Mover para…' com árvore de pastas (reusa Sidebar, não drag-and-drop)
- Flags preservadas (Seen copiado; MOVE preserva, fallback COPY copia flags)
- Offline: fila em imap_outbox, replay com reconcile (padrão flag_outbox)
- Mensagem com toggle pendente: move carrega o op pendente junto

### the agent's Discretion
- Trash folder detection strategy (SPECIAL-USE vs name match vs auto-detect)
- Exact undo toast copy and placement

</decisions>

<code_context>
## Existing Code Insights

### Reusable Assets
- `SessionManager::set_seen_in` reconnect-retry template (manager.rs) for new verbs
- `flag_outbox` durable queue + RFC 4549 replay (worker.rs) for imap_outbox pattern
- Sidebar folder tree (Sidebar.tsx) for the move-destination menu
- `SyncGate` single-flight, tombstone/prune machinery

### Established Patterns
- UID-only addressing everywhere; BODY.PEEK audit; per-folder sync state
- Tauri commands with `Option<String>` mailbox defaulting to INBOX
- Optimistic UI + pending wash + rollback-on-reject (MessageList toggle)

### Integration Points
- `SyncSession` trait (imap/mod.rs): add store_deleted/expunge/copy/move/append verbs
- `start_sync` must move under manager leases before destructive ops (research precondition)
- Poll + manual refresh loop over all folders (SyncStatus requestSync)

</code>

<specifics>
## Specific Ideas

- Trash auto-detect: probe SPECIAL-USE/attributes, fall back to name match, CREATE 'Trash' as last resort
- Move must hold the lease across double-SELECT so EXPUNGE can't hit the wrong folder

</specifics>

<deferred>
## Deferred Ideas

None — discussion stayed within phase scope

</deferred>
