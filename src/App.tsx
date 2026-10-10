import { useState, useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";
import LoginForm from "./components/LoginForm";
import SyncStatus from "./components/SyncStatus";
import { OutboxBadgeWired } from "./components/OutboxBadge";
import { ClassifyStatusWired } from "./components/ClassifyStatus";
import SendStatus from "./components/SendStatus";
import MailboxView from "./components/MailboxView";
import { IconCap, IconSparkle, IconArrowLeft } from "./components/icons";
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
  const [manualDisconnect, setManualDisconnect] = useState(false);
  const [studentName, setStudentName] = useState("");

  useEffect(() => {
    if (!isTauriRuntime()) return;
    if (connected || autoConnecting) return;
    if (manualDisconnect) return;

    let cancelled = false;

    invoke<SavedCredentials | null>("load_credentials")
      .then(async (creds) => {
        if (!cancelled && creds) {
          setStudentName(creds.username);
          setAutoConnecting(true);
          try {
            const serverCfg = await invoke<ServerConfig | null>("load_server_config");
            if (!cancelled && serverCfg) {
              await invoke<ConnectSummary>("connect_account", {
                host: serverCfg.host,
                port: serverCfg.port,
                security: serverCfg.security,
                username: creds.username,
                password: creds.password,
              });
              if (!cancelled) setConnected(true);
            }
          } catch {
            if (!cancelled) setAutoConnecting(false);
          }
          if (!cancelled) setAutoConnecting(false);
        }
      })
      .catch(() => {
        if (!cancelled) setAutoConnecting(false);
      });

    return () => {
      cancelled = true;
    };
  }, [connected, manualDisconnect]);

  function handleConnect() {
    setConnected(true);
    setManualDisconnect(false);
  }

  function handleDisconnect() {
    setConnected(false);
    setManualDisconnect(true);
  }

  return (
    <div className="edu-shell">
      <header className="edu-topbar">
        <div className="edu-brand">
          <span className="edu-brand-mark" aria-hidden="true">
            <IconCap size={26} />
          </span>
          <span>
            <span className="edu-brand-name">Sistema de Gestão de E-mails</span>
            <br />
            <span className="edu-brand-tag">
              <span className="dot" aria-hidden="true" />
              Painel Principal
            </span>
          </span>
        </div>
        <div className="edu-topbar-actions">
          {connected ? (
            <>
              <span className="edu-chip" title="Estudante conectado">
                <IconSparkle size={15} />
                {studentName ? `Olá, ${studentName.split("@")[0]}` : "Sala de aula aberta"}
              </span>
              <button type="button" className="btn-ghost" onClick={handleDisconnect}>
                <IconArrowLeft size={15} />
                Trocar conta
              </button>
            </>
          ) : (
            <span className="edu-chip edu-chip--accent">Acesso do estudante</span>
          )}
        </div>
      </header>

      {outsideDesktop && (
        <p role="alert" className="edu-alert">
          O SGE Edu roda na janela do app — abra com <code>npm run tauri dev</code> e use a janela
          do aplicativo, não o endereço do navegador.
        </p>
      )}

      {!connected && autoConnecting && (
        <p className="auto-connect-note" role="status" aria-live="polite">
          Abrindo sua sala de aula…
        </p>
      )}

      {!connected && !autoConnecting ? (
        <>
          <LoginForm onConnect={handleConnect} />
          <div style={{ marginTop: 16 }}>
            <SyncStatus />
            <OutboxBadgeWired mailbox="INBOX" />
            <ClassifyStatusWired />
            <SendStatus mailbox="INBOX" onRetry={(id) => console.error(`retry ${id}`)} />
          </div>
        </>
      ) : connected ? (
        <MailboxView />
      ) : null}

      <footer className="edu-footer">
        <span>SGE Edu · sua caixa institucional inteligente</span>
        <span aria-hidden="true">·</span>
        <span>Mensagens ficam no servidor · leitura segura</span>
      </footer>
    </div>
  );
}

export default App;
