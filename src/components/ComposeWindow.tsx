// Phase 37 — Compose Window (Thunderbird-style)
// Modal/standalone window with To/Cc/Bcc/Subject/Body/attachments
// Reuses Phase 14 DraftEditor component (header fields)
// Sends via queue_send (Phase 35) → M10 send_queue → SMTP (Phase 35)
// BCC envelope-only (stripped from rendered .eml headers)
// Draft saves via Phase 14 APPEND contract; Send via Phase 35 transport

import React, { useState } from "react";
import DraftEditor, { type DraftEditorProps } from "./DraftEditor";

type ComposeWindowProps = {
  onClose: () => void;
  onSend?: () => void;
  onSaveDraft?: () => void;
};
export const ComposeWindow: React.FC<ComposeWindowProps> = ({ onClose, onSend, onSaveDraft }) => {
  const [attachments, setAttachments] = useState<File[]>([]);

  // Reuse Phase 14 DraftEditor (To/Cc/Bcc/Subject/Body/attachments + Send/Save Draft)
  // ComposeWindow hosts DraftEditor with initial blank state
  const draftEditorProps: DraftEditorProps = {
    draftId: null,
    initial: null,
    onSaved: () => {
      onSaveDraft?.();
    },
    onDiscarded: () => {
      onClose();
    },
    onClose,
  };

  return (
    <div className="compose-window">
      <h2>New Message</h2>
      <DraftEditor {...draftEditorProps} />
      {attachments.length > 0 && (
        <div className="attachments">
          {attachments.map((file, i) => (
            <span key={i}>{file.name}</span>
          ))}
        </div>
      )}
      <div className="compose-actions">
        <button type="button" onClick={() => { onSend?.(); onClose(); }}>Send</button>
        <button type="button" onClick={onSaveDraft}>Save Draft</button>
        <button type="button" onClick={onClose}>Cancel</button>
      </div>
    </div>
  );
};
