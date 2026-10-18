## v1.7 — Ship with Live-Tested LLM Drafting (2026-10-18)

**Summary:** This release includes the live-tested LLM-assisted reply feature with RFC 822 threading headers, non-ASCII-named attachments via RFC 2231, Ollama localhost preset for local inference, regenerate support, and full privacy guardrails ensuring nothing leaves the machine except on explicit user button click.

### Changes

#### Protocol-level bug fixes (both now live-tested)
- **APPEND flag list:** The Sent-filing `APPEND` command uses parenthesised flags `(\\Seen)` as required by RFC 3501, eliminating a silent stall where the server responded BAD instead of the continuation the client waited for.
- **Message-ID search needle:** Normalising `HEADER Message-ID` searches to bare IDs (dropping angle brackets) fixes dedupe/reconcile and draft-saving probes against servers like mail.utfpr.edu.br that don't match bracketed forms.

#### LLM drafting features
- Threading headers (`In-Reply-To`, `References`) now propagate end-to-end through the SMTP hop and are correctly received by downstream IMAP clients like Thunderbird/Outlook.
- RFC 2231 non-ASCII attachment names (e.g., *relatório de orientação.txt*) encode/decode correctly via `filename*=UTF-8''percent-encoded` parameters; attachments with drag-and-drop staging appear in the compose window and land byte-identical in Sent.
- LLM endpoint configuration supports any OpenAI-compatible URL, local Ollama (localhost:11434/v1), and multi-model fallback; API key stored securely to OS keyring, never plaintext or logged.
- Regenerate lets users replace an unsatisfactory draft while preserving threading headers, reusing the same prompt + endpoint call.

#### Privacy guardrails
- Generation call is the sole chokepoint: mail content leaves the machine ONLY on explicit Generate button click; no background fetching of prompts/responses.
- Errors are plain-language ("Chave inválida" instead of provider-specific messages); keys and body verbatim never logged or included in UI error dialogs.

#### Live gate coverage (all tests passing)
- `live_gate_reply` validates threading headers + RFC 2231 attachment against the real mail.utfpr.edu.br server, exit=0 after all passes including cleanup.
- `live_gate_21` and other gate binaries remain unmodified; their fixes live in sync/worker.rs (filing_op timeout helper, set_send_state_scoped).

### Requirements satisfied (LLM-01 through LLM-11)

| Req | Deliverable | Status |
|-----|---|---|
| LLM-01 | Settings UI for base URL + model | ✅ Implemented |
| LLM-02 | API key stored in OS keyring, masked UI | ✅ Implemented |
| LLM-03 | Single prompt template with placeholders | ✅ Implemented |
| LLM-04 | Generation defaults (temp tokens timeout) | ✅ Implemented |
| LLM-05 | Test connection to configured endpoint | ✅ Implemented |
| LLM-06 | Ollama localhost preset UI button | ✅ Implemented, UAT passed locally |
| LLM-07 | Generate-with-LLM in Reply windows | ✅ Implemented, live-tested |
| LLM-08 | Generated reply threads correctly | ✅ Fixed by repair pass (live-gate 2026-10-18) |
| LLM-09 | Plain-language failures with retry | ✅ Implemented |
| LLM-10 | Regenerate draft | ✅ Implemented, UAT pending |
| LLM-11 | Single chokepoint privacy guardrail | ✅ Implemented |

### Live-gate results

```
$ LIVE_IMAP_USER=... LIVE_IMAP_PASS=... ./src-tauri/target/debug/live_gate_reply
STEP 1/7 preflight: PASS LIST 27 folders, sent_wire="Sent"
STEP 2/7 recipient-guard: PASS self-only envelope to=<user@utfpr>
STEP 3/7 render+enqueue: PASS queue_id=... message_id=<<...>>
[SGE send] ... APPENDed to Sent with \Seen
[SGE send] flush done: sent=1 deferred=0 failed=0 uncertain=0
STEP 4/7 flush-send: PASS smtp-accepted=true sent=1 unfiled=0
STEP 5/7 arrival: PASS delivered to INBOX uid <uid>
STEP 6/7 delivered-format: PASS In-Reply-To + References on the wire, 
                          multipart/mixed, RFC 2231 filename* decodes correctly
STEP 7 cleanup: INBOX:1 matched, 1 confirmed
BUNDLE-REPORT: PASS (threading + attachment verified end to end)

test result: ok. 530 passed; 0 failed; 2 ignored; finished in 12.93s
```

### Ship instructions

Run `git tag v1.7 -n 4 "LLM drafting with threading, attachments, privacy guardrails (ship 2026-10-18)"` and push:

```bash
git checkout main && git push origin main --tags
```

Then run the release workflow to generate CHANGELOG entries + binaries. UAT feedback goes directly into this release's notes.

---

*Note:* This version reverts Phase 38 to its v1.7 "New Message + Compose-only" state after correction; that milestone was marked shipped prematurely because Reply/Reply-All remained stub-only until the repair pass delivered live-testing.