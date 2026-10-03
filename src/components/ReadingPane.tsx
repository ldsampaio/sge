import "./MailboxView.css";
import type { MessageRow } from "../types";

interface ReadingPaneProps {
  selectedMessage: MessageRow | null;
}

/**
 * Reading pane slot. In Phase 3 this is a placeholder — message body
 * rendering (HTML sanitization, plaintext fallback, attachments) lands
 * in Phase 4. When a message is selected, we show its preview text as a
 * non-navigable hint.
 */
export default function ReadingPane({ selectedMessage }: ReadingPaneProps) {
  if (!selectedMessage) {
    return (
      <aside className="reading-pane" aria-label="Reading pane">
        <p className="reading-placeholder">Select a message to read</p>
      </aside>
    );
  }

  return (
    <aside className="reading-pane" aria-label="Reading pane">
      <div className="reading-content">
        <p className="reading-preview">
          <em>Message preview (full reader coming in Phase 4):</em>
        </p>
        <p className="reading-preview-text">{selectedMessage.preview || "(no preview available)"}</p>
      </div>
    </aside>
  );
}
