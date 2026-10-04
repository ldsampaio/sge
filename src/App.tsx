import { useState, useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";
import LoginForm from "./components/LoginForm";
import SecuritySelector from "./components/SecuritySelector";
import SyncStatus from "./components/SyncStatus";
import MailboxView from "./components/MailboxView";
import "./App.css";

function isTauriRuntime(): boolean {
  const w = window as unknown as Record<string, unknown>;
  return w["__TAURI_INTERNALS__"] !== undefined || w["__TAURI__"] !== undefined;
}

interface SavedCredentials {
  username: string;
  password: string;
}

interface ServerConfig {
  host: string;
  port: number;
  security: string;
}

interface ConnectSummary {
  selected_mailbox: string;
  uid_validity: number;
  exists: number;
}

type SecurityMode = "implicit_tls" | "starttls" | "plain";

function App() {
  const outsideDesktop = !isTauriRuntime();
  const [mode, setMode] = useState<SecurityMode>("implicit_tls");
  const [port, setPort] = useState(993);
  const [connected, setConnected] = useState(false);
  const [autoConnecting, setAutoConnecting] = useState(false);

  useEffect(() => {
    // Persist the user's security+port choice across sessions.
    const saved = localStorage.getItem("sge-security-mode");
    if (saved === "starttls" || saved === "plain") setMode(saved);
    const savedPort = localStorage.getItem("sge-security-port");
    if (savedPort) setPort(Number(savedPort));
  }, []);

  // Phase 5: Auto-connect on launch when saved credentials exist.
  // Loads from keyring → connect_account → start_sync.
  useEffect(() => {
    if (!isTauriRuntime()) return;
    if (connected || autoConnecting) return;

    let cancelled = false;

    invoke<SavedCredentials | null>("load_credentials")
      .then(async (creds) => {
        if (!cancelled && creds) {
          // Credentials exist — try to load server config + auto-connect.
          setAutoConnecting(true);
          try {
            const serverCfg = await invoke<ServerConfig | null>(
              "load_server_config",
            );
            if (!cancelled && serverCfg) {
              await invoke<ConnectSummary>("connect_account", {
                host: serverCfg.host,
                port: serverCfg.port,
                security: serverCfg.security,
                username: creds.username,
                password: creds.password,
              });
              await invoke("start_sync", {
                mailbox: "INBOX",
                limit: 0,
              });
              if (!cancelled) setConnected(true);
            }
          } catch {
            // Auto-connect failed — show login form so the user can retry.
            // (No silent plaintext fallback — WR-02.)
            if (!cancelled) setAutoConnecting(false);
          }
          if (!cancelled) setAutoConnecting(false);
        }
      })
      .catch(() => {
        // Keyring unavailable (not a Tauri runtime, or Secret Service
        // missing) — show the login form. Never a plaintext fallback.
        if (!cancelled) setAutoConnecting(false);
      });

    return () => {
      cancelled = true;
    };
  }, []);

  function handleModeChange(next: SecurityMode) {
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
      {!connected && autoConnecting && (
        <p className="auto-connect-note">Connecting to your mail…</p>
      )}
      {!connected && !autoConnecting ? (
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
