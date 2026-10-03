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
  return w["__TAURI_INTERNALS__"] !== undefined || w["__TAURI__"] !== undefined;
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
 * SGE login form (locked decisions D-server-prefill, D-security-ui,
 * D-show-hide, D-single-connect, D-failure, D-remember, D-demo).
 *
 * Single Connect button: validates, connects, SELECTs INBOX, and shows the
 * real connection state. Failures surface the backend typed error verbatim
 * with a manual Retry button — this component never retries on its own
 * (no timers, no reconnect effects).
 */
export default function LoginForm() {
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

  // Reload remembered credentials once on launch (no auto-connect here —
  // auto-login stays in Phase 5; the user still presses Connect).
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
        // Keyring unavailable: stay memory-only, fields keep their defaults.
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
    // WR-04: guard here so NaN/out-of-range ports get the plain-language
    // message instead of an opaque serde/u16 deserialization failure.
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
    <div>
      <form
        onSubmit={(e) => {
          e.preventDefault();
          void connect();
        }}
      >
        <label>
          Server
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
        <label>
          Username
          <input
            id="username-input"
            value={username}
            onChange={(e) => setUsername(e.currentTarget.value)}
            autoComplete="username"
          />
        </label>
        <label>
          Password
          <input
            id="password-input"
            type={showPassword ? "text" : "password"}
            value={password}
            onChange={(e) => setPassword(e.currentTarget.value)}
            autoComplete="current-password"
          />
        </label>
        <button
          type="button"
          aria-label={showPassword ? "Hide password" : "Show password"}
          onClick={() => setShowPassword((v) => !v)}
        >
          {showPassword ? "Hide" : "Show"}
        </button>
        <label>
          <input
            type="checkbox"
            checked={rememberMe}
            onChange={(e) => setRememberMe(e.currentTarget.checked)}
          />
          Remember me (save username and password in the OS keyring)
        </label>
        <button type="submit" disabled={status.kind === "connecting"}>
          {status.kind === "connecting" ? "Connecting…" : "Connect"}
        </button>
        <button type="button" onClick={() => void forgetSaved()}>
          Forget saved login
        </button>
      </form>
      {keyringHint && <p>{keyringHint}</p>}
      {forgetNote && <p>{forgetNote}</p>}

      {status.kind === "connecting" && <p>Connecting…</p>}

      {status.kind === "success" && (
        <div>
          <p>
            Connected: {status.summary.selected_mailbox} (
            {status.summary.exists} messages, UIDVALIDITY{" "}
            {status.summary.uid_validity})
          </p>
          {status.keyringNote && <p>{status.keyringNote}</p>}
        </div>
      )}

      {status.kind === "error" && (
        <div>
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
