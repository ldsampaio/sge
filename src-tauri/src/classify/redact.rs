//! Redaction sanitizer (Phase 16, Plan 16-03) — the ONE privacy gate.
//!
//! THEOREM: every later phase MUST route classifier evidence through
//! [`redact_input`] and every displayed/logged justification through
//! [`filter_output`]. Labels store category IDs only (schema-enforced), so
//! this module is the only place secret-shaped text is handled.
//!
//! Replacement keeps the KIND (`[REDACTED:code]`) so the classifier still
//! sees that a code exists without seeing the code itself.

use std::sync::OnceLock;

use regex::Regex;

struct Rules {
    /// (pattern, kind) pairs, applied in order.
    pairs: Vec<(Regex, &'static str)>,
    /// Paranoid backstop for output: any 6+ digit run.
    digit_run: Regex,
}

fn rules() -> &'static Rules {
    static CELL: OnceLock<Rules> = OnceLock::new();
    CELL.get_or_init(|| {
        let pair = |pat: &str, kind: &'static str| (Regex::new(pat).unwrap(), kind);
        // ORDER MATTERS: longest/most-specific first. The phone pattern can
        // match digit slices inside longer runs, so barcode + CPF run before
        // it; otherwise a boleto line gets shredded into phone-shaped pieces
        // and the barcode rule never fires.
        Rules {
            pairs: vec![
                // Passwords: `senha[: =] <value>`, password/passwd variants.
                pair(
                    r"(?i)(senhas?|password|passwd|pwd)\s*[:=\-]\s*\S+",
                    "password",
                ),
                // Labeled codes: `código/token/otp/verificação[: =] <value>`.
                pair(
                    r"(?i)(c[oó]digos?|tokens?|otps?|verifica[cç][aã]o|chave\s+de\s+acesso)\s*[:=\-]\s*\S+",
                    "code",
                ),
                // Bare codes: `código 739201` (value MUST contain a digit —
                // otherwise benign `código de conduta` would be eaten).
                pair(
                    r"(?i)\b(c[oó]digos?|tokens?|otps?|verifica[cç][aã]o)\s+(\S*\d\S*)",
                    "code",
                ),
                // Bare passwords: `senha Temp1234` (digit-shaped only).
                pair(
                    r"(?i)\b(senhas?|password|passwd|pwd)\s+(\S*\d\S*)",
                    "password",
                ),
                // API keys / secrets / bearer: separator OR bare space
                // (`bearer XXX`). Over-redaction beats leakage.
                pair(
                    r"(?i)\b(api[_-]?keys?|secrets?|bearer)\b\s*(?:[:=\-]\s*)?\S+",
                    "secret",
                ),
                // Barcodes / long digit runs typical of boletos (44–48 digits).
                pair(r"\b\d{44,48}\b", "barcode"),
                // CPF formatted + bare 11-digit (word-boundary guarded).
                pair(r"\b\d{3}\.\d{3}\.\d{3}-\d{2}\b", "cpf"),
                pair(r"\b\d{11}\b", "cpf"),
                // RA / matrícula: `RA 12345`, `matrícula: 2024...`.
                pair(r"(?i)\b(ra|matr[ií]cula)\s*[:=\-]?\s*\d[\d.\-]*", "ra"),
                // BR phones: (41) 99999-0000, 41999990000, +55 41 ....
                // `(?:55)?` — optional country code (NOT `55?`, which would
                // require a literal 5).
                pair(
                    r"\+?(?:55)?\s*\(?\d{2}\)?\s*\d{4,5}[-.\s]?\d{4}\b",
                    "phone",
                ),
            ],
            digit_run: Regex::new(r"\b\d{6,}\b").unwrap(),
        }
    })
}

/// Scrub secrets from classifier INPUT (evidence text). Keeps sentence shape
/// and a `[REDACTED:<kind>]` marker per hit.
pub fn redact_input(text: &str) -> String {
    let mut out = text.to_string();
    for (re, kind) in &rules().pairs {
        let replacement = format!("[REDACTED:{kind}]");
        out = re.replace_all(&out, replacement.as_str()).into_owned();
    }
    out
}

/// Backstop for classifier OUTPUT (justifications, log lines): applies the
/// same pairs plus a paranoid any-6+-digit-run rule. Justification strings
/// are transient UI only (never persisted) — this keeps even those clean.
pub fn filter_output(text: &str) -> String {
    let once = redact_input(text);
    rules()
        .digit_run
        .replace_all(&once, "[REDACTED:digits]")
        .into_owned()
}

/// True when no digit run of 6+ survives (quick hygiene assertion for tests
/// and for the Phase 17 worker's pre-inference gate).
pub fn looks_clean(text: &str) -> bool {
    !rules().digit_run.is_match(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: no secret substring from `secrets` may survive `f`.
    fn assert_scrubbed(f: fn(&str) -> String, text: &str, secrets: &[&str]) {
        let out = f(text);
        for s in secrets {
            assert!(
                !out.contains(s),
                "secret {s:?} survived:\n{out}"
            );
        }
    }

    #[test]
    fn password_reset_mail_scrubbed() {
        let mail = "Olá! Sua senha temporária é senha: Xk9#mQ2! Use o código: 481516 para ativar. \
            Token de acesso token=abc123def456. Não compartilhe.";
        for f in [redact_input, filter_output] {
            assert_scrubbed(f, mail, &["Xk9#mQ2!", "481516", "abc123def456"]);
        }
        let out = redact_input(mail);
        assert!(out.contains("[REDACTED:password]"));
        assert!(out.contains("[REDACTED:code]"));
    }

    #[test]
    fn boleto_barcode_scrubbed() {
        let boleto = "Fatura vencida. Linha digitável 23793381286007000123456000000012347650000012345 \
            no valor de R$ 123,45.";
        for f in [redact_input, filter_output] {
            assert_scrubbed(
                f,
                boleto,
                &["23793381286007000123456000000012347650000012345"],
            );
        }
        assert!(redact_input(boleto).contains("[REDACTED:barcode]"));
    }

    #[test]
    fn cpf_and_phone_scrubbed() {
        let sig = "Atenciosamente, João — CPF 123.456.789-09, fone (41) 99999-1234, \
            matrícula RA 2023123456.";
        for f in [redact_input, filter_output] {
            assert_scrubbed(
                f,
                sig,
                &["123.456.789-09", "(41) 99999-1234", "2023123456"],
            );
        }
    }

    #[test]
    fn api_key_and_bearer_scrubbed() {
        let leak = "Deploy: api_key=sk-live-9f8e7d6c5b4a e bearer AAAABBBBCCCCDDDD.";
        for f in [redact_input, filter_output] {
            assert_scrubbed(f, leak, &["sk-live-9f8e7d6c5b4a", "AAAABBBBCCCCDDDD"]);
        }
    }

    #[test]
    fn output_backstop_catches_bare_digit_runs() {
        // A 6-digit code with NO label is invisible to redact_input's
        // labeled pairs — filter_output's paranoid rule must still catch it.
        let sneaky = "seu número é 739201, guarde bem";
        assert!(redact_input(sneaky).contains("739201"));
        assert!(!filter_output(sneaky).contains("739201"));
        assert!(looks_clean(&filter_output(sneaky)));
    }

    #[test]
    fn benign_text_untouched() {
        let ok = "Reunião do colegiado na quinta às 14h na sala B12. Pauta: calendário 2026.";
        assert_eq!(redact_input(ok), ok);
        assert_eq!(filter_output(ok), ok);
        assert!(looks_clean(ok));
    }

    #[test]
    fn benign_code_words_without_digits_survive() {
        // `código de conduta` has no digit-shaped value — must NOT redact.
        let ok = "Leia o código de conduta antes da prova.";
        assert_eq!(redact_input(ok), ok);
        let bare = "digite o código 739201 para continuar";
        assert!(!redact_input(bare).contains("739201"));
    }

    #[test]
    fn short_numbers_survive() {
        // Years, room numbers, small counts must NOT be redacted (signal!).
        let ok = "Prova dia 12 de março, sala 204, peso 3.";
        assert_eq!(filter_output(ok), ok);
    }

    #[test]
    fn kinds_preserved_as_signal() {
        let out = redact_input("código: 555666 e senha: segredo!");
        assert!(out.contains("[REDACTED:code]") && out.contains("[REDACTED:password]"));
    }
}
