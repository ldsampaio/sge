import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import "./App.css";

interface ConnectSummary {
  selected_mailbox: string;
  uid_validity: number;
  exists: number;
}

function App() {
  const [server, setServer] = useState("mail.utfpr.edu.br");
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [status, setStatus] = useState("");

  async function connect() {
    setStatus("Connecting…");
    try {
      const summary = await invoke<ConnectSummary>("connect_account", {
        host: server,
        port: 993,
        security: "implicit_tls",
        username,
        password,
      });
      setStatus(
        `Connected: ${summary.selected_mailbox} (${summary.exists} messages, UIDVALIDITY ${summary.uid_validity})`,
      );
    } catch (err) {
      setStatus(`Connection failed: ${String(err)}`);
    }
  }

  return (
    <main className="container">
      <h1>SGE</h1>
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
          />
        </label>
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
            type="password"
            value={password}
            onChange={(e) => setPassword(e.currentTarget.value)}
            autoComplete="current-password"
          />
        </label>
        <button type="submit">Connect</button>
      </form>
      <p>{status}</p>
    </main>
  );
}

export default App;
