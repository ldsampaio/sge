import { useState, useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";
import LoginForm from "./components/LoginForm";
import SyncStatus from "./components/SyncStatus";
import MailboxView from "./components/MailboxView";
import "./App.css";

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

function isTauriRuntime(): boolean {
  const w = window as unknown as Record<string, unknown>;
  return w["__TAURI_INTERNALS__"] !== undefined || w["__TAURI__"] !== undefined;
}

function App() {
  const outsideDesktop = !isTauriRuntime();
  const [connected, setConnected] = useState(false);
  const [autoConnecting, setAutoConnecting] = useState(false);

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
  }, [connected, autoConnecting]);

  function handleConnect() {
    setConnected(true);
  }

  function handleDisconnect() {
    setConnected(false);
  }

  return (
    <main className="container">
      <h1>Sistema de Gestão de E-mail</h1>
      {outsideDesktop && (
        <p role="alert">
          SGE is running outside its desktop window — launch with `npm run
          tauri dev` and use the app window, not the browser URL.
        </p>
      )}
      {!connected && autoConnecting && (
        <p className="auto-connect-note">Conectando sua caixa de entrada…</p>
      )}
      {!connected && !autoConnecting ? (
        <>
          <SyncStatus />
          <LoginForm onConnect={handleConnect} />
        </>
      ) : (
        <>
          <button
            type="button"
            className="disconnect-btn"
            onClick={handleDisconnect}
            title="Disconnect and return to login"
          >
            ← Mudar conta
          </button>
          <MailboxView />
        </>
      )}
    </main>
  );
}

export default App;
