export type SecurityModeValue = "implicit_tls" | "starttls" | "plain";

export const DEFAULT_PORTS: Record<SecurityModeValue, number> = {
  implicit_tls: 993,
  starttls: 143,
  plain: 143,
};

interface SecuritySelectorProps {
  mode: SecurityModeValue;
  port: number;
  onModeChange: (mode: SecurityModeValue) => void;
  onPortChange: (port: number) => void;
}

/**
 * Security mode selector (locked decisions D-security-ui).
 *
 * Three modes mapping 1:1 to the backend `SecurityMode` enum values:
 * implicit_tls (SSL/TLS, default) / starttls / plain (localhost only, with a
 * visible warning). The port auto-fills from the mode and stays editable
 * under the Advanced row.
 */
export default function SecuritySelector({
  mode,
  port,
  onModeChange,
  onPortChange,
}: SecuritySelectorProps) {
  return (
    <fieldset>
      <legend>Security</legend>
      <label>
        <input
          type="radio"
          name="security"
          value="implicit_tls"
          checked={mode === "implicit_tls"}
          onChange={() => onModeChange("implicit_tls")}
        />
        SSL/TLS (port 993) — recommended
      </label>
      <label>
        <input
          type="radio"
          name="security"
          value="starttls"
          checked={mode === "starttls"}
          onChange={() => onModeChange("starttls")}
        />
        STARTTLS (port 143)
      </label>
      <label>
        <input
          type="radio"
          name="security"
          value="plain"
          checked={mode === "plain"}
          onChange={() => onModeChange("plain")}
        />
        Unencrypted (local server only)
      </label>
      {mode === "plain" && (
        <p role="alert">
          Warning: unencrypted mode sends your password without protection and
          works for localhost servers only. Remote hosts are always refused.
        </p>
      )}
      <details>
        <summary>Advanced</summary>
        <label>
          Port
          <input
            id="port-input"
            type="number"
            min={1}
            max={65535}
            value={port}
            onChange={(e) => onPortChange(Number(e.currentTarget.value))}
          />
        </label>
      </details>
    </fieldset>
  );
}
