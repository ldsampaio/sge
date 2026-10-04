import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import SecuritySelector, {
  DEFAULT_PORTS,
  type SecurityModeValue,
} from "./SecuritySelector";

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

// Plain-browser guard: when the frontend runs under plain Vite (browser URL)
// instead of the Tauri WebView, `invoke()` throws a raw TypeError about
// `window.__TAURI_INTERNALS__` being undefined. Detect the missing runtime
// before any invoke call and surface plain language instead (D-failure).
const OUTSIDE_DESKTOP_MESSAGE =
  "SGE is running outside its desktop window — launch with `npm run tauri dev` and use the app window, not the browser URL.";

function isTauriRuntime(): boolean {
  const w = window as unknown as Record<string, unknown>;
  return w["__TAURI_INTERNALS__"] !== undefined || w["__TAURI__"] !== null;
}

/** Translate a raw missing-runtime TypeError into the plain-language message. */
function toPlainError(err: unknown): string {
  const text = String(err);
  if (text.includes("__TAURI_INTERNALS__") || text.includes("__TAURI__")) {
    return OUTSIDE_DESKTOP_MESSAGE;
  }
  return text;
}

/**
 * SGE login form.
 *
 * Field order: Username → Password → Server → IMAP Security → Connect →
 * Forget saved login → Remember me.
 *
 * Single Connect button: validates, connects, SELECTs INBOX, and shows the
 * real connection state. Failures surface the backend typed error verbatim
 * with a manual Retry button — this component never retries on its own
 * (no timers, no reconnect effects).
 */
export default function LoginForm({ onConnect }: { onConnect?: () => void }) {
  const [server, setServer] = useState("mail.utfpr.edu.br");
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [showPassword, setShowPassword] = useState(false);
  const [rememberMe, setRememberMe] = useState(false);
  const [mode, setMode] = useState<SecurityModeValue>("implicit_tls");
  const [port, setPort] = useState(DEFAULT_PORTS.implicit_tls);
  const [status, setStatus] = useState<Status>({ kind: "idle" });
  // IN-03: visible when the keyring cannot be read on launch.
  const [keyringHint, setKeyringHint] = useState<string | null>(null);
  const [forgetNote, setForgetNote] = useState<string | null>(null);

  // Auto-connect only runs in App.tsx; LoginForm just fills remembered values.
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
            "Saved login unavailable — keyring locked? Continuing without remembered credentials.",
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
        message: "Enter the server, username, and password first.",
      });
      return;
    }
    if (!Number.isInteger(port) || port < 1 || port > 65535) {
      setStatus({
        kind: "error",
        message: "Port must be a number between 1 and 65535.",
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
      // Persist server config so start_sync can reconnect.
      try {
        await invoke("save_server_config", {
          host: server.trim(),
          port,
          security: mode,
        });
      } catch {
        // Best-effort; the server config is also re-entered on each connect.
      }
      let keyringNote: string | null = null;
      if (rememberMe) {
        try {
          await invoke("save_credentials", {
            username: username.trim(),
            password,
          });
          keyringNote = "Username and password saved in the OS keyring.";
          // WR-10: the secret now lives in the keyring — drop it from JS
          // memory instead of lingering in state indefinitely.
          setPassword("");
        } catch (err) {
          // Keyring locked/headless: fall back to a memory-only session with
          // an explanatory note — never a silent file.
          keyringNote = `Remember-me unavailable (${toPlainError(err)}). Continuing with a memory-only session.`;
        }
      } else {
        // WR-02: opting out must revoke, not just ignore — a previously
        // saved entry would otherwise be reloaded on next launch.
        try {
          await invoke("clear_credentials");
        } catch {
          // Best-effort cleanup; a stale entry is merely inconvenient, and
          // the Forget button below offers a manual retry.
        }
      }
      setStatus({ kind: "success", summary, keyringNote });
      onConnect?.();
    } catch (err) {
      setStatus({ kind: "error", message: toPlainError(err) });
    }
  }

  // WR-02: in-app revocation for saved credentials.
  async function forgetSaved() {
    if (!isTauriRuntime()) {
      setForgetNote(OUTSIDE_DESKTOP_MESSAGE);
      return;
    }
    try {
      await invoke("clear_credentials");
      setRememberMe(false);
      setPassword("");
      setForgetNote("Saved login forgotten — keyring entry cleared.");
    } catch (err) {
      setForgetNote(`Could not clear the saved login (${toPlainError(err)}).`);
    }
  }

  const isTlsError =
    status.kind === "error" && status.message.includes("TLS verification failed");

  return (
    <div className="login-form-container">
      <form
        onSubmit={(e) => {
          e.preventDefault();
          void connect();
        }}
      >
        {/* 1. Username */}
        <label>
          Usuario
          <input
            id="username-input"
            value={username}
            onChange={(e) => setUsername(e.currentTarget.value)}
            autoComplete="username"
          />
        </label>

        {/* 2. Password with built-in show/hide toggle */}
        <label>
          Senha
          <div className="password-field">
            <input
              id="password-input"
              type={showPassword ? "text" : "password"}
              value={password}
              onChange={(e) => setPassword(e.currentTarget.value)}
              autoComplete="current-password"
            />
            <button
              type="button"
              className="password-toggle"
              aria-label={showPassword ? "Ocultar senha" : "Mostrar senha"}
              onClick={() => setShowPassword((v) => !v)}
            >
              {showPassword ? "Ocultar" : "Mostrar"}
            </button>
          </div>
        </label>

        {/* 3. Server */}
        <label>
          Servidor
          <input
            id="server-input"
            value={server}
            onChange={(e) => setServer(e.currentTarget.value)}
            autoComplete="off"
          />
        </label>

        {/* 4. IMAP Security options */}
        <SecuritySelector
          mode={mode}
          port={port}
          onModeChange={handleModeChange}
          onPortChange={setPort}
        />

        {/* 5. Connect button */}
        <button type="submit" disabled={status.kind === "connecting"}>
          {status.kind === "connecting" ? "Conectando…" : "Conectar"}
        </button>

        {/* 6. Forget saved login */}
        <button type="button" onClick={() => void forgetSaved()}>
          Esquecer login salvo
        </button>

        {/* 7. Other options */}
        <label className="checkbox-row">
          <input
            type="checkbox"
            checked={rememberMe}
            onChange={(e) => setRememberMe(e.currentTarget.checked)}
          />
          Lembrar me (salvar usuario e senha no keyring do SO)
        </label>
      </form>

      {keyringHint && <p className="hint">{keyringHint}</p>}
      {forgetNote && <p className="hint">{forgetNote}</p>}

      {status.kind === "connecting" && <p className="status">Conectando…</p>}

      {status.kind === "success" && (
        <div className="status-success">
          <p>
            Conectado: {status.summary.selected_mailbox} ({status.summary.exists} messages, UIDVALIDITY{" "}
            {status.summary.uid_validity})
          </p>
          {status.keyringNote && <p className="keyring-note">{status.keyringNote}</p>}
        </div>
      )}

      {status.kind === "error" && (
        <div className="status-error">
          <p role="alert">Connection failed: {status.message}</p>
          {isTlsError && (
            <p role="alert">
              Warning: the server certificate could not be verified. SGE will
              not bypass this check silently — retrying re-verifies the
              certificate strictly.
            </p>
          )}
          <button type="button" onClick={() => void connect()}>
            {isTlsError ? "I understand — retry anyway" : "Retry"}
          </button>
        </div>
      )}
    </div>
  );
}
