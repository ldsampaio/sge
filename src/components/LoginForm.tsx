import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import SecuritySelector, { DEFAULT_PORTS, type SecurityModeValue } from "./SecuritySelector";
import { IconBook, IconInbox, IconHelp, IconSparkle } from "./icons";

interface ConnectSummary {
  selected_mailbox: string;
  uid_validity: number;
  exists: number;
}

interface SavedCredentials {
  username: string;
  password: string;
}

type Status =
  | { kind: "idle" }
  | { kind: "connecting" }
  | { kind: "success"; summary: ConnectSummary; keyringNote: string | null }
  | { kind: "error"; message: string };

const OUTSIDE_DESKTOP_MESSAGE =
  "O SGE Edu roda na janela do app — abra com `npm run tauri dev` e use a janela do aplicativo, não o endereço do navegador.";

function isTauriRuntime(): boolean {
  const w = window as unknown as Record<string, unknown>;
  return w["__TAURI_INTERNALS__"] !== undefined || w["__TAURI__"] !== null;
}

function toPlainError(err: unknown): string {
  const text = String(err);
  if (text.includes("__TAURI_INTERNALS__") || text.includes("__TAURI__")) {
    return OUTSIDE_DESKTOP_MESSAGE;
  }
  return text;
}

export default function LoginForm({ onConnect }: { onConnect?: () => void }) {
  const [server, setServer] = useState("mail.utfpr.edu.br");
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [showPassword, setShowPassword] = useState(false);
  const [rememberMe, setRememberMe] = useState(false);
  const [mode, setMode] = useState<SecurityModeValue>("implicit_tls");
  const [port, setPort] = useState(DEFAULT_PORTS.implicit_tls);
  const [status, setStatus] = useState<Status>({ kind: "idle" });
  const [keyringHint, setKeyringHint] = useState<string | null>(null);
  const [forgetNote, setForgetNote] = useState<string | null>(null);

  useEffect(() => {
    if (!isTauriRuntime()) {
      setKeyringHint(OUTSIDE_DESKTOP_MESSAGE);
      return;
    }
    let cancelled = false;
    invoke<SavedCredentials | null>("load_credentials")
      .then((saved) => {
        if (!cancelled && saved) {
          setUsername(saved.username);
          setPassword(saved.password);
          setRememberMe(true);
        }
      })
      .catch(() => {
        if (!cancelled) {
          setKeyringHint(
            "Login salvo indisponível — cofre bloqueado? Continuando sem credenciais lembradas.",
          );
        }
      });
    return () => {
      cancelled = true;
    };
  }, []);

  function handleModeChange(next: SecurityModeValue) {
    setMode(next);
    setPort(DEFAULT_PORTS[next]);
  }

  async function connect() {
    if (!isTauriRuntime()) {
      setStatus({ kind: "error", message: OUTSIDE_DESKTOP_MESSAGE });
      return;
    }
    if (!server.trim() || !username.trim() || !password) {
      setStatus({
        kind: "error",
        message: "Preencha servidor, usuário e senha para entrar na sala.",
      });
      return;
    }
    if (!Number.isInteger(port) || port < 1 || port > 65535) {
      setStatus({
        kind: "error",
        message: "A porta deve ser um número entre 1 e 65535.",
      });
      return;
    }
    setStatus({ kind: "connecting" });
    try {
      const summary = await invoke<ConnectSummary>("connect_account", {
        host: server.trim(),
        port,
        security: mode,
        username: username.trim(),
        password,
      });
      try {
        await invoke("save_server_config", {
          host: server.trim(),
          port,
          security: mode,
        });
      } catch {
        // Best-effort
      }
      let keyringNote: string | null = null;
      if (rememberMe) {
        try {
          await invoke("save_credentials", {
            username: username.trim(),
            password,
          });
          keyringNote = "Usuário e senha guardados no cofre do sistema.";
          setPassword("");
        } catch (err) {
          keyringNote = `Lembrete indisponível (${toPlainError(err)}). Seguindo com sessão só em memória.`;
        }
      } else {
        try {
          await invoke("clear_credentials");
        } catch {
          // Best-effort cleanup
        }
      }
      setStatus({ kind: "success", summary, keyringNote });
      onConnect?.();
    } catch (err) {
      setStatus({ kind: "error", message: toPlainError(err) });
    }
  }

  async function forgetSaved() {
    if (!isTauriRuntime()) {
      setForgetNote(OUTSIDE_DESKTOP_MESSAGE);
      return;
    }
    try {
      await invoke("clear_credentials");
      setRememberMe(false);
      setPassword("");
      setForgetNote("Login salvo esquecido — entrada do cofre apagada.");
    } catch (err) {
      setForgetNote(`Não foi possível apagar o login salvo (${toPlainError(err)}).`);
    }
  }

  const isTlsError = status.kind === "error" && status.message.includes("TLS verification failed");

  return (
    <div className="login-form-container">
      <div className="edu-login-grid">
        <section className="edu-hero" aria-label="Boas-vindas ao SGE Edu">
          <span className="edu-hero-eyebrow">
            <IconSparkle size={14} />
            Sua Caixa de E-mails Inteligente
          </span>
          <h1>
            Sua caixa institucional,
            <br />
            organizada para facilitar a vida.
          </h1>
          <p className="lead">
            O SGE Edu transforma seu e-mail em um auxiliar inteligente capaz de responder e-mails,
            assinar documentos automaticamente entre outras funcionalidades.
          </p>
          <ul className="edu-hero-list">
            <li>
              <span className="edu-hero-icon" aria-hidden="true">
                <IconInbox size={18} />
              </span>
              <span>
                <strong>Trilha de entrada</strong>
                <span>Caixa de entrada vira fila de leitura: o que é novo fica em destaque.</span>
              </span>
            </li>
            <li>
              <span className="edu-hero-icon" aria-hidden="true">
                <IconBook size={18} />
              </span>
              <span>
                <strong>Rascunho de Respostas</strong>
                <span>O agente lê o e-mail e prepara uma resposta como rascunho.</span>
              </span>
            </li>
            <li>
              <span className="edu-hero-icon" aria-hidden="true">
                <IconHelp size={18} />
              </span>
              <span>
                <strong>Integração Automatizada</strong>
                <span>Mensagens que solicitam a assinatura de documentos utilizam assinaturas via SEI ou gov.br
                  após sua aprovação.</span>
              </span>
            </li>
          </ul>
        </section>

        <section className="edu-access-card" aria-label="Acesso do estudante">
          <h2>Acesso</h2>
          <p className="edu-access-sub">
            Configure abaixo o acesso a sua caixa de e-mails.
          </p>
          <form
            onSubmit={(e) => {
              e.preventDefault();
              void connect();
            }}
          >
            <label>
              Usuário institucional
              <input
                id="username-input"
                value={username}
                onChange={(e) => setUsername(e.currentTarget.value)}
                autoComplete="username"
                placeholder="nome.sobrenome"
              />
            </label>

            <label>
              Senha
              <span className="password-field">
                <input
                  id="password-input"
                  type={showPassword ? "text" : "password"}
                  value={password}
                  onChange={(e) => setPassword(e.currentTarget.value)}
                  autoComplete="current-password"
                  placeholder="••••••••"
                />
                <button
                  type="button"
                  className="password-toggle"
                  aria-label={showPassword ? "Ocultar senha" : "Mostrar senha"}
                  onClick={() => setShowPassword((v) => !v)}
                >
                  {showPassword ? "Ocultar" : "Mostrar"}
                </button>
              </span>
            </label>

            <label>
              Servidor da Caixa de E-mails
              <input
                id="server-input"
                value={server}
                onChange={(e) => setServer(e.currentTarget.value)}
                autoComplete="off"
              />
            </label>

            <SecuritySelector
              mode={mode}
              port={port}
              onModeChange={handleModeChange}
              onPortChange={setPort}
            />

            <button type="submit" disabled={status.kind === "connecting"}>
              {status.kind === "connecting" ? "Abrindo a caixa…" : "Entrar"}
            </button>
          </form>

          {keyringHint && <p className="hint">{keyringHint}</p>}
          {forgetNote && <p className="hint">{forgetNote}</p>}

          {status.kind === "connecting" && (
            <p className="status" role="status">
              Conectando à caixa...
            </p>
          )}

          {status.kind === "success" && (
            <div className="status-success">
              <p>
                Caixa aberta: {status.summary.selected_mailbox} ({status.summary.exists} mensagens,
                UIDVALIDITY {status.summary.uid_validity})
              </p>
              {status.keyringNote && <p>{status.keyringNote}</p>}
            </div>
          )}

          {status.kind === "error" && (
            <div className="status-error">
              <p role="alert">Não foi possível entrar: {status.message}</p>
              {isTlsError && (
                <p role="alert">
                  O certificado do servidor não pôde ser verificado. O SGE Edu não ignora essa
                  verificação — tentar de novo verifica tudo outra vez.
                </p>
              )}
              <button type="button" className="btn" onClick={() => void connect()}>
                {isTlsError ? "Entendi — tentar de novo" : "Tentar de novo"}
              </button>
            </div>
          )}

          <div className="login-sub-actions">
            <button type="button" className="btn-secondary" onClick={() => void forgetSaved()}>
              Esquecer login salvo
            </button>
            <label className="checkbox-row">
              <input
                type="checkbox"
                checked={rememberMe}
                onChange={(e) => setRememberMe(e.currentTarget.checked)}
              />
              Lembrar de mim (guardar usuário e senha no cofre do sistema)
            </label>
          </div>
        </section>
      </div>
    </div>
  );
}
