import { useState, useEffect } from "react";
import LoginForm from "./components/LoginForm";
import SecuritySelector from "./components/SecuritySelector";
import SyncStatus from "./components/SyncStatus";
import MailboxView from "./components/MailboxView";
import "./App.css";

function isTauriRuntime(): boolean {
  const w = window as unknown as Record<string, unknown>;
  return w["__TAURI_INTERNALS__"] !== undefined || w["__TAURI__"] !== undefined;
}

function App() {
  const outsideDesktop = !isTauriRuntime();
  // Persist the user's security+port choice across sessions.
  const [mode, setMode] = useState<"implicit_tls" | "starttls" | "plain">("implicit_tls");
  const [port, setPort] = useState(993);
  // connected = true after a successful connect_account (Phase 3: mailbox view)
  const [connected, setConnected] = useState(false);

  useEffect(() => {
    const saved = localStorage.getItem("sge-security-mode");
    if (saved === "starttls" || saved === "plain") setMode(saved);
    const savedPort = localStorage.getItem("sge-security-port");
    if (savedPort) setPort(Number(savedPort));
  }, []);

  function handleModeChange(next: "implicit_tls" | "starttls" | "plain") {
    setMode(next);
    setPort(next === "implicit_tls" ? 993 : 143);
    localStorage.setItem("sge-security-mode", next);
    localStorage.setItem("sge-security-port", String(next === "implicit_tls" ? 993 : 143));
  }

  function handleConnect() {
    setConnected(true);
  }

  function handleDisconnect() {
    setConnected(false);
  }

  return (
    <main className="container">
      <h1>SGE</h1>
      {outsideDesktop && (
        <p role="alert">
          SGE is running outside its desktop window — launch with `npm run
          tauri dev` and use the app window, not the browser URL.
        </p>
      )}
      {!connected ? (
        <>
          <SecuritySelector mode={mode} port={port} onModeChange={handleModeChange} onPortChange={setPort} />
          <LoginForm onConnect={handleConnect} />
          <SyncStatus />
        </>
      ) : (
        <>
          <button
            type="button"
            className="disconnect-btn"
            onClick={handleDisconnect}
            title="Disconnect and return to login"
          >
            ← Change account
          </button>
          <MailboxView />
        </>
      )}
    </main>
  );
}

export default App;
