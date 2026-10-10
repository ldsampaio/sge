//! Evidence pipeline (Phase 17, Plan 17-01): quote-strip →
//! signal-extract → budget-fit → redact.
//!
//! The classifier NEVER sees full bodies: subject (≤200) + snippet (≤1000)
//! + lightweight signals (sender domain, reply/attachment flags). Redaction
//! is the LAST step before return, and `looks_clean` is asserted by the
//! worker's pre-inference gate in debug builds.

use super::redact;

/// Subject budget (chars). Subjects are the highest-signal field.
pub const SUBJECT_MAX: usize = 200;
/// Snippet budget (chars). Snippets derive from cached preview — no forced
/// body FETCH (if the Phase 17 eval disappoints, escalate to
/// fetch-body-for-pending-only; worker change, not architecture change).
pub const SNIPPET_MAX: usize = 1000;

/// Quote prefixes stripped line-wise (`>`, `|`, `On … wrote:`, `-- `).
fn strip_quotes(text: &str) -> String {
    let mut kept = Vec::new();
    for line in text.lines() {
        let t = line.trim_start();
        if t.starts_with('>') || t.starts_with('|') {
            continue;
        }
        let lower = t.to_lowercase();
        if lower.starts_with("em ") && lower.contains("escreveu:") {
            continue;
        }
        if lower.starts_with("on ") && lower.contains("wrote:") {
            continue;
        }
        if t == "--" || t.starts_with("-- ") {
            break; // signature separator: drop the rest
        }
        kept.push(line);
    }
    kept.join("\n")
}

fn fit(s: &str, max: usize) -> String {
    let mut out: String = s.chars().take(max).collect();
    if s.chars().count() > max {
        out.push('…');
    }
    out
}

/// Build redacted evidence text from cached fields. Pure + total: no I/O,
/// no network. Output is ALWAYS `redact_input`-clean.
pub fn build_evidence(
    subject: &str,
    snippet: &str,
    from_domain: &str,
    has_attachments: bool,
    is_reply: bool,
) -> String {
    let mut parts = Vec::new();
    parts.push(format!("Assunto: {}", fit(subject.trim(), SUBJECT_MAX)));
    if !from_domain.trim().is_empty() {
        parts.push(format!("De: {}", fit(from_domain.trim(), 80)));
    }
    let mut signals = Vec::new();
    if has_attachments {
        signals.push("tem anexo");
    }
    if is_reply {
        signals.push("é resposta");
    }
    if !signals.is_empty() {
        parts.push(format!("Sinais: {}", signals.join(", ")));
    }
    let body = fit(strip_quotes(snippet).trim(), SNIPPET_MAX);
    if !body.is_empty() {
        parts.push(format!("Trecho: {body}"));
    }
    let raw = parts.join("\n");
    // Input redaction strips LABELED secrets; bare digit runs survive here
    // on purpose — they are transient classifier signal (never persisted;
    // labels store IDs only). The paranoid digit backstop applies to
    // DISPLAYED text via filter_output, not to inference input.
    redact::redact_input(&raw)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_quotes_and_signatures() {
        let snippet = "Segue o boleto em anexo.\n> On Mon, João wrote:\n> manda o boleto\n-- \nJoão Silva";
        let ev = build_evidence("Boleto", snippet, "utfpr.edu.br", true, true);
        assert!(ev.contains("Segue o boleto em anexo."));
        assert!(!ev.contains("manda o boleto"));
        assert!(!ev.contains("João Silva"));
        assert!(ev.contains("tem anexo") && ev.contains("é resposta"));
    }

    #[test]
    fn budgets_hold() {
        let long = "x".repeat(5000);
        let ev = build_evidence(&long, &long, "d", false, false);
        let subject_line = ev.lines().next().unwrap();
        assert!(subject_line.chars().count() <= "Assunto: ".len() + SUBJECT_MAX + 1);
        assert!(ev.chars().count() <= 200 + 1000 + 200);
    }

    #[test]
    fn secrets_never_survive_evidence() {
        let ev = build_evidence(
            "Recuperação de senha",
            "sua senha: Temp1234! e o código 739201 chegará",
            "utfpr.edu.br",
            false,
            false,
        );
        assert!(!ev.contains("Temp1234!"));
        assert!(!ev.contains("739201"));
        // Displayed text gets the extra backstop; evidence itself keeps
        // only labeled-secret scrubbing (transient, never persisted).
        assert!(crate::classify::redact::looks_clean(
            &crate::classify::redact::filter_output(&ev)
        ));
    }

    #[test]
    fn empty_fields_still_yield_subject() {
        let ev = build_evidence("Oi", "", "", false, false);
        assert!(ev.contains("Assunto: Oi"));
    }
}
