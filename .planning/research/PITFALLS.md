# Pitfalls Research: v1.5 Additions to SGE

**Domain:** Adding IDLE push, CONDSTORE/QRESYNC, Sieve export, learning-from-overrides,
per-category auto-confirm, rule import/export, override history, batch dry-run +
retry-failed, compose hardening, and live-credential test runs to an existing
single-session IMAP client with offline replay queues
**Researched:** 2026-10-13
**Confidence:** MEDIUM (codebase-grounded claims HIGH; crate-ecosystem claims LOW–MEDIUM
per `query classify-confidence --provider websearch` = LOW; RFC-grounded claims MEDIUM)

> System being extended: Rust + Tauri v2 + React + SQLite. Single-session
> `imap::manager::SessionManager` (one guarded connection, lease + reconnect-retry +
> drain pattern reused since Phase 10/11). Poll sync (5-min UI timer → one guarded
> `start_sync` path, `SyncGate` single-flight). `imap_outbox` offline replay queue with
> RFC 4549 drop rules + UIDVALIDITY epoch gating. BODY.PEEK-only invariant (never sets
> `\Seen` on fetch). Ammonia HTML sanitizer + sandboxed iframe. Sandbox live-test
> contract (`SGE-Test-<run-id>` folder, own-address-only sends, pre-flight guard,
> exit-2 SKIP when no creds). Redaction discipline (never log secrets / senhas /
> códigos). `Auto/` classifier tree with user-confirmed single-file, unconfirmed batch.

## Critical Pitfalls

### Pitfall 1: IDLE holds the only connection hostage — second-connection races with SessionManager leases

**What goes wrong:**
IDLE is a blocking command: from `IDLE` until `DONE`, the connection can do nothing
else. `async-imap` exposes this as an `IdleHandle` (`extensions/idle.rs`) whose
`wait()` owns the session. If IDLE runs on the single `SessionManager` connection,
every poll sync, flag-STORE replay, MOVE, APPEND, and STATUS blocks until IDLE is
torn down — or worse, a lease request steals the connection mid-IDLE and corrupts
both operations. The naive alternative (open a second raw connection for IDLE
beside the manager) bypasses every invariant the manager exists for: no
reconnect-retry, no lease serialization, two writers racing on the same selected
mailbox, and credential handling outside the keyring-guarded path.

**Why it happens:**
Developers treat IDLE as "just another command" and either (a) park it on the
manager connection, or (b) spin a second `Client` in a Tokio task because "IDLE
needs its own connection" — both patterns look correct in isolation. (b) is
especially tempting because the manager's lease API has no notion of a
long-lived subscription.

**How to avoid:**
- Decide the ownership model at plan time, not implementation time: either
  IDLE-loop owns a *dedicated* connection created **through** the SessionManager
  (manager gains a `subscribe_idle()` primitive with the same reconnect-retry +
  backoff discipline, never a raw `Client::new` in sync code), or IDLE runs as a
  supervised child task that can *only* signal "mailbox changed → run normal poll
  sync through the manager" and never issues STORE/FETCH itself.
- IDLE must be an *advisory wakeup*, not a second sync engine: on EXISTS/RECENT
  untagged responses it triggers the existing guarded `start_sync` path; all
  UIDVALIDITY checks, outbox replay, and flag reconcile stay in the one engine.
- Poll stays the fallback and the authority: server without IDLE capability,
  dropped IDLE (>29 min without DONE per RFC 2177 — servers time out ~29 min),
  NAT timeouts, and the `STARTTLS IDLE` capability-parse path (see session.rs
  capability tests) must all degrade to the current 5-min poll with zero
  behavior change. DONE + re-IDLE heartbeat on a timer, not fire-and-forget.
- Serialize DONE vs lease acquisition: stopping IDLE (sending DONE) and starting
  a sync must be one atomic transition, otherwise untagged responses arriving
  between DONE-ack and SELECT race the sync's UIDVALIDITY snapshot.

**Warning signs:**
- New `TcpStream`/`TlsConnector` construction outside `imap::manager`.
- `idle()` called on a session obtained from a normal lease without a DONE path
  on every exit (panic, cancel, reconnect).
- Tests that pass with MockSession but no test kills the IDLE connection mid-wait
  and asserts reconnect + no duplicate sync.

**Phase to address:**
The IDLE phase (first sync-protocol phase). Gate the phase on a design decision
record: connection-ownership model + DONE/reconnect state machine, before code.

---

### Pitfall 2: MODSEQ vs UIDVALIDITY interplay — CONDSTORE fast path silently trusts what the epoch gate rejects

**What goes wrong:**
CONDSTORE/QRESYNC introduce a second change-detection axis (MODSEQ /
HIGHESTMODSEQ) alongside the existing UIDVALIDITY epoch gate. Failure modes:
(a) client stores HIGHESTMODSEQ per mailbox but doesn't invalidate it on
UIDVALIDITY change → after a server-side mailbox rebuild, stale MODSEQs map to
*different messages* and flag updates land on wrong UIDs; (b) server advertises
CONDSTORE but not QRESYNC (or vice versa) and the code assumes both; (c) `ENABLE
CONDSTORE` changes untagged FETCH semantics (CONDSTORE-enabled FETCH returns
MODSEQ) which breaks the existing FETCH parsers that assume RFC 3501 shapes
(this codebase already carries one parser-gap scar: imap-proto 0.16 vs NAMESPACE);
(d) `async-imap 0.11`'s extension surface for CONDSTORE/MODSEQ is thin — hand-rolled
command strings bypass the typed API and its tests, re-introducing the exact class
of bug the NAMESPACE tripwire guards against.

**Why it happens:**
MODSEQ feels like a drop-in "sync faster" flag, and the happy path (same server,
no UIDVALIDITY change, CONDSTORE advertised) works in every manual test. The
interplay only breaks on rare server events (mailbox rebuild, migration, Dovecot
vs Zimbra capability differences) — exactly the events the epoch gate was built
for but the new code path doesn't consult.

**How to avoid:**
- Rule: **UIDVALIDITY check first, MODSEQ second, always.** The CONDSTORE fast
  path is entered only after the epoch gate passes; on UIDVALIDITY mismatch the
  stored MODSEQ is discarded with the rest of the mailbox state (extend the
  RFC 4549 drop rule in `store/mod.rs` to cover a new `highestmodseq` column —
  it is epoch-scoped state, same as cached UIDs).
- Capability-gate per connection: parse CAPABILITY fresh per session (don't cache
  across reconnects); CONDSTORE-only servers get flag-diff-via-FETCH-MODSEQ,
  QRESYNC servers get `SELECT ... (QRESYNC ...)` + VANISHED handling, neither
  gets the old poll path removed. ` mail.utfpr.edu.br` capability set must be
  recorded live before planning (unknown today).
- Any raw `ENABLE`/`SELECT (CONDSTORE)`/untagged-MODSEQ parsing gets a regression
  tripwire test in the style of the NAMESPACE one: a fake-server transcript with
  MODSEQ-carrying FETCH + VANISHED responses that must parse.
- Scope cut: if live capability probing shows the real server lacks CONDSTORE,
  ship IDLE-only and keep poll; do not emulate MODSEQ client-side (a local
  "highest seen" counter is not a MODSEQ and silently diverges).

**Warning signs:**
- New MODSEQ storage without a UIDVALIDITY-bump invalidation path.
- `HIGHESTMODSEQ` compared across sessions without re-SELECT.
- Raw command strings (`"ENABLE CONDSTORE"`) outside the typed client API with
  no transcript test.

**Phase to address:**
The CONDSTORE/QRESYNC phase, *after* the IDLE phase (IDLE changes connection
lifetimes; MODSEQ semantics depend on stable SELECT behavior). Requires a live
capability probe as phase entry criterion.

---

### Pitfall 3: Auto-confirm trust erosion — per-category auto-confirm silently widens the blast radius of classifier errors

**What goes wrong:**
v1.3's trust contract is explicit: single-email always user-confirmed, batch
unconfirmed only as an explicit whole-account op with progress + report.
Per-category auto-confirm ("auto-file all `Financeiro/*` without asking") punches
a standing hole in that contract. A misclassification then MOVEs mail with no
human in the loop, repeatedly, into the same wrong folder — and because the move
is server-side (MOVE verb), the damage persists across resyncs and is visible to
other clients. Users discover it weeks later ("where did my invoices go?") and
trust in the whole classifier collapses. Learning-from-overrides amplifies this:
if an auto-confirmed misfile is never overridden (user never sees it), the
learner treats silence as approval and reinforces the error.

**Why it happens:**
Per-category toggles feel like a small UX addition ("just remember my confirms"),
and each individual auto-file looks correct in testing. The failure is
statistical and slow — error rate × volume × time-to-discovery — so no single
test or demo catches it.

**How to avoid:**
- Auto-confirm must be **opt-in per category, default off**, with the UI showing
  the category's recent precision (confirms vs overrides from override history)
  at the toggle site. Suggested guardrail: auto-confirm engages only after N
  consecutive user confirms of that category (e.g. 10) and *disengages*
  automatically on the next override (circuit breaker back to manual).
- Every auto-confirmed MOVE is journaled exactly like batch (folder, UID, epoch,
  reversible) and surfaced in a periodic digest ("this week auto-filed 42 → 2
  overridden") so silence never equals invisibility.
- Learning-from-overrides must consume *explicit* signals only (user confirm /
  override / manual move); auto-confirmed-but-unseen mail is training-neutral,
  never positive. Otherwise the feedback loop is self-reinforcing.
- Undo for auto-filed mail must be one click from the digest, reusing the Phase 10
  MOVE machinery (UID-only + epoch gating), not a new verb path.

**Warning signs:**
- A global "enable auto-confirm" switch instead of per-category with precision
  display.
- Learner training on all filed mail without distinguishing confirmed vs
  auto-filed-unseen.
- No auto-disengage on override; no digest/undo surface.

**Phase to address:**
The classifier follow-through phase (learning + auto-confirm + override history
as one coherent unit — splitting them across phases lets the feedback loop ship
without its governor). Override history + precision metrics first, auto-confirm
second, learner wiring last.

---

### Pitfall 4: Secret leakage in live runs — real creds escape via logs, transcripts, and debug dumps

**What goes wrong:**
Live validation with real credentials (`src-tauri/.live-test.env`, OS keyring)
collides with debugging instincts: IMAP protocol transcripts (tagged commands
with `LOGIN user pass`), full message dumps, error contexts with `Debug` on
structs holding passwords, shell `set -x` echoing the env file, CI artifacts, or
a pasted terminal session. One `LOGIN` line in a committed log or a `.live-test.env`
in `git add -A` compromises the user's real mailbox password. The redaction
sanitizer exists for classifier justifications/logs — live-test output paths
(transcripts, failure dumps, UAT notes) are new surfaces it may not cover.

**Why it happens:**
Live runs are rare, high-attention events; developers crank up verbosity
(`RUST_LOG=trace`, raw transcript capture) to diagnose failures and forget to
scrub before sharing. The exit-2 SKIP contract means live scripts mostly ran
*without* creds in this environment — the first real-creds run is exactly when
untested logging paths execute.

**How to avoid:**
- Extend the redaction sanitizer (or a dedicated transcript scrubber) to the
  live-test output path *before* the first real-creds run: scrub `LOGIN` /
  `AUTHENTICATE` arguments, `Authorization` headers, and env-file contents in
  all transcript captures and failure dumps; unit-test the scrubber with a fake
  transcript containing a password.
- Pre-flight checklist as code, not docs: the sandbox guard asserts
  `.live-test.env` is git-ignored (fail the run if `git check-ignore` fails),
  asserts no `trace`-level transcript sink writes to a file outside `/tmp`,
  and `LOGIN` never appears in any persisted artifact (grep the run log
  post-run as a verification step).
- Least-privilege habit: live runs use the account owner's own credentials only,
  sends to own address only (existing contract); never add "share this failing
  transcript" output that includes raw commands — ship a `--redacted` dump flag
  as the *only* dump path.
- Rotate the credential after the milestone if the password was ever pasted,
  typed into a non-keyring prompt, or logged anywhere; document this in the
  live-run plan.

**Warning signs:**
- `println!("{:?}", ...)` on session/client/config structs in new live-test code.
- Transcript capture without a scrub pass; `LOGIN` appearing in any test fixture.
- `.live-test.env` referenced by a path that isn't asserted git-ignored.

**Phase to address:**
The live-validation phase (first phase that touches real creds). The scrubber +
pre-flight assertions are entry criteria — implemented and unit-tested before
the first live run, verified by grepping run artifacts for secrets.

---

### Pitfall 5: Sieve export is a second product wearing a feature costume — server capability unknown, semantics don't map 1:1

**What goes wrong:**
Sieve export means: translating local smart rules (keyword/sender → folder) into
Sieve scripts (RFC 5228) and uploading via ManageSieve (RFC 5804, typically port
4190) — a *new protocol client* (no Rust ManageSieve crate surfaced; hand-rolled
TLS + SASL + script PUT/activate), against a server whose ManageSieve support is
currently **unknown** (`mail.utfpr.edu.br` is 993-only for IMAP; nothing known
about 4190 or Sieve support — could be Zimbra with its own filter API instead).
Semantic mismatches pile up: local rules reference local-only concepts (secondary
category labels, `Auto/` tree state, SQLite-only history) with no Sieve
equivalent; folder names need Sieve string escaping + `fileinto` + `mailbox`
extension handling; `require` capability negotiation per script; activating a
script *replaces* server-side behavior for mail the local classifier also
processes → double-filing or fight loops (server files to X, client MOVEs to Y
on next sync, user sees flapping).

**Why it happens:**
"Export rules" sounds like serialization, and Sieve syntax for simple
`if header :contains "from" → fileinto` cases is genuinely simple. The complexity
is all in the last mile: transport, capability negotiation, activation semantics,
and coexistence with the local classifier — none visible in a syntax demo.

**How to avoid:**
- Plan-time feasibility probe (cheap, before any code): port-4190 reachability +
  ManageSieve greeting from the real server, `require`-capability list, and
  whether activation is even permitted on the account. If the server doesn't
  offer ManageSieve, **defer explicitly** — do not build a generic uploader
  against an untestable endpoint.
- If viable: export is one-way, explicit, and non-authoritative — a "Generate
  Sieve script" preview (copy-pasteable text + dry-run diff) ships before any
  upload path; upload requires separate explicit confirmation and never
  auto-syncs on rule edits.
- Coexistence rule: categories/folders managed by an active server script must be
  excluded from local auto-file (or vice versa) with a visible "managed by
  Sieve" badge; otherwise the two engines fight over the same mail.
- Scope the Sieve subset ruthlessly: only `fileinto` on From/Subject contains
  (what local rules express); no `vacation`, no `reject`, no regex, no script
  *import* (parsing arbitrary server scripts back into local rules is a parser
  project of its own — note rule import/export locally is JSON, a different,
  safe thing).

**Warning signs:**
- ManageSieve client code written before a recorded 4190 probe of the real server.
- Round-trip import ("read server script into rules") scoped in the same phase.
- No coexistence story between server-side filing and local classifier MOVEs.

**Phase to address:**
Dedicated Sieve phase placed *late* (after live validation proves server access
patterns), with the feasibility probe as its entry gate — probe fails → phase
converts to "export-as-text-file only" with no upload path. PROJECT.md already
flags this ("validate complexity at plan time").

---

### Pitfall 6: Offline replay queues + push-triggered syncs = duplicate or reordered STORE/MOVE execution

**What goes wrong:**
Today: poll ticks are single-flighted through `SyncGate`; refresh arriving
mid-sync gets `None` (busy ticks skip). IDLE wakeups add a second, bursty trigger
source. Failure modes: (a) IDLE fires EXISTS during an in-progress outbox replay
→ second sync starts replaying the same `imap_outbox` rows (double STORE of Seen
flags, double MOVE — second MOVE fails or moves the wrong message post-UID
shift); (b) retry-failed (new manual trigger) races an automatic sync over the
same failed rows; (c) dry-run and real execution share code paths and dry-run
accidentally marks rows in-flight; (d) batch dry-run enumerates a UID set that
goes stale before the user confirms (new arrivals shift UIDs — UID-indexed
batches, not UIDVALIDITY+UID addressed, silently target wrong mail).

**Why it happens:**
Each trigger (poll timer, manual refresh, IDLE wakeup, retry-failed button,
batch confirm) is added as "just call start_sync" and each looks serialized in
isolation. Concurrency only appears under real timing: push bursts, slow
networks, user impatience double-clicking retry.

**How to avoid:**
- All triggers — poll, manual, IDLE wakeup, retry-failed, batch confirm — funnel
  through the *existing* single-flight gate (`SyncGate`), extending it rather
  than bypassing it; IDLE wakeup coalesces (a flag, not a queue: "dirty since
  last sync") so a burst of 50 EXISTS events yields one sync.
- Outbox rows carry a state machine (`pending → in-flight → acked/failed`) with
  the in-flight claim in the same SQLite transaction that reads them; a second
  sync seeing in-flight rows skips them (lease semantics at the row level,
  mirroring the SessionManager lease pattern).
- Dry-run executes strictly read-only: enforced by taking only shared/read
  snapshots (no outbox claim, no MOVE/STORE verbs reachable — ideally a type-level
  split: dry-run context lacks the `&mut` sync handle that verbs require);
  batch confirm re-resolves UIDs against current UIDVALIDITY + UIDNEXT
  immediately before executing and aborts/translates on mismatch, showing the
  user what changed.
- Retry-failed re-queues as new pending rows (or resets to pending in one
  transaction) and then triggers the normal path — never a parallel executor.

**Warning signs:**
- A new `start_sync` caller that doesn't go through `SyncGate`.
- Outbox claim-and-execute in separate transactions (crash window = duplicate
  replay on restart).
- Batch preview storing array indices or sequence numbers instead of
  (UIDVALIDITY, UID) pairs.

**Phase to address:**
The IDLE phase (trigger funnel + coalescing) and the batch/retry phase
(row-level claims, dry-run read-only enforcement, UID re-resolution). Both need
a concurrency stress test: fake server emitting untagged EXISTS mid-replay +
double-clicked retry.

---

### Pitfall 7: Rule import/export round-trip drift — exported JSON reimports as a different rule

**What goes wrong:**
Export looks done when a JSON file appears; the pitfall is semantic drift:
exported rules lose fields the importer doesn't restore (folder role vs name,
case-sensitivity flags, escaped patterns, rule ordering/priority, enabled state,
per-category auto-confirm bindings), so export → import → export doesn't fixpoint.
Worse: importing rules referencing folders that don't exist (or `Auto/` tree
names from another account) creates dangling rules that either crash matching or
silently never fire. Import without validation is also an injection surface:
pattern strings become regex/IMAP SEARCH fragments — a crafted import file can
inject pathological regexes (ReDoS on the matcher) or SEARCH syntax the server
rejects mid-sync.

**Why it happens:**
Export and import are built as two separate serializers ("write JSON", "read
JSON") rather than one versioned schema with a round-trip property test. Folder
references feel like plain strings until the folder doesn't exist.

**How to avoid:**
- One versioned schema (`{"version":1,"rules":[...]}`), one module, with a
  round-trip property test: export(import(x)) == x and import(export(r)) ≡ r
  (semantic equality) over generated rule sets including non-ASCII names,
  escaped quotes, disabled rules, and priority ties.
- Import is validate-then-stage-then-commit: schema version check → field
  validation (pattern compiles, folder exists or is explicitly creatable with
  user confirm, priority unique-ified) → preview diff ("3 new, 1 conflicts with
  rule #5") → user confirms conflicts (reusing the existing
  conflict=user-confirm rule UX from v1.4) → commit. Never direct insert.
- Patterns compile in a sandboxed matcher with timeout/length caps; reject (with
  a clear message) anything that doesn't compile instead of storing a
  never-firing rule. Imported auto-confirm bindings default OFF regardless of
  exported value (trust must be re-earned on the new machine — see Pitfall 3).
- Unknown schema versions fail closed with "exported by a newer SGE" rather
  than best-effort partial import.

**Warning signs:**
- Separate export/import code with no shared schema type.
- No round-trip test; no `version` field in the format.
- Import path that inserts rows without folder-existence + pattern-compile checks.

**Phase to address:**
The classifier follow-through phase (rule import/export + override history).
Round-trip property test is the phase's headline acceptance criterion.

---

### Pitfall 8: Compose hardening re-verifies with live sends that violate the sandbox (wrong recipient, duplicate send on retry)

**What goes wrong:**
Compose hardening (send/drafts/reply/reply-all over the v1.2 pipelines) tempts
"real" verification: sending actual test mail. Failure modes: (a) a reply-all
test leaks outside the sandbox (Cc list from a real thread → real people get
test mail); (b) retry/backoff verification double-sends (the outbox exactly-once
machinery — Message-ID dedupe + Sent filing — is bypassed by a test that calls
the SMTP verb directly); (c) draft APPEND tests leave orphan drafts when cleanup
runs before the APPEND ack; (d) reply quoting includes sensitive content that
then lands in logs (collision with the redaction discipline).

**Why it happens:**
"Just send one to make sure it works" bypasses the sandbox contract that every
prior live phase honored. Reply-all is the sharp edge: its recipient list comes
from *data* (the original message), not from a constant, so a hardcoded
"own-address-only" guard that works for new-compose doesn't automatically cover
reply-all.

**How to avoid:**
- Pre-flight guard extended, not rewritten: recipient allowlist = {own address}
  enforced on the *resolved* recipient list (To+Cc+Bcc after reply-all
  expansion), aborting the run on any out-of-sandbox address — with a dedicated
  test using a fixture message containing foreign Cc addresses asserting the
  abort.
- Exactly-once verification through the pipeline, not around it: retry tests go
  through `imap_outbox`/SMTP queue with Message-ID dedupe asserted (two queued
  sends, one delivery); never call the raw send verb twice and eyeball it.
- Draft tests reuse the `SGE-Test-<run-id>` + post-run cleanup discipline from
  v1.4 (one-copy invariant asserted before cleanup, cleanup verified after).
- Quoted-body fixtures are synthetic (no real mail content) so reply tests can't
  leak sensitive data into logs even on failure.

**Warning signs:**
- Test helpers that accept arbitrary recipient lists without allowlist checks.
- Retry tests asserting "no crash" instead of "exactly one delivery".
- Reply-all code path resolving recipients differently from the guarded path.

**Phase to address:**
The compose-hardening phase. Entry: sandbox pre-flight covering expanded
recipients; exit: exactly-once + cleanup assertions green, live or record-replay.

---

## Technical Debt Patterns

| Shortcut | Immediate Benefit | Long-term Cost | When Acceptable |
|----------|-------------------|----------------|-----------------|
| Second raw IMAP connection for IDLE beside SessionManager | IDLE works in a day | Two writers, no reconnect discipline, invariant bypass (Pitfall 1) | Never — route through the manager |
| Client-side "MODSEQ" counter emulating CONDSTORE | Fast-path demo without server support | Silent divergence from server truth (Pitfall 2) | Never |
| Global auto-confirm switch | One checkbox ships fast | Trust erosion with no per-category precision or circuit breaker (Pitfall 3) | Never — per-category + auto-disengage minimum |
| Training learner on all filed mail | More training data immediately | Silence-as-approval reinforces errors (Pitfall 3) | Never — explicit signals only |
| Hand-rolled ManageSieve client before 4190 probe | Feels like progress | Untestable code against possibly nonexistent endpoint (Pitfall 5) | Only after probe succeeds |
| Sieve round-trip import in the same phase as export | "Complete" story | Arbitrary-script parser project smuggled into a phase (Pitfall 5) | Never in v1.5 — export-text only at most |
| Dry-run sharing the mutable sync handle | Code reuse | Accidental mutation from a "read-only" path (Pitfall 6) | Never — type-level split |
| Import inserting rules without validation | Import "works" on happy path | Dangling rules, ReDoS patterns, trust smuggling (Pitfall 7) | Never |
| Live tests calling SMTP verb directly | Quick verification | Exactly-once bypass, sandbox violations (Pitfall 8) | Never — through the pipeline |
| Trace-level transcript files in /tmp during live runs | Easier debugging | Secret leakage via persisted artifacts (Pitfall 4) | Only with scrubber + post-run grep gate |

## Integration Gotchas

| Integration | Common Mistake | Correct Approach |
|-------------|---------------|------------------|
| async-imap IDLE (`extensions/idle.rs`) | `wait()` on the manager connection blocks all ops; DONE missing on cancel paths | Dedicated manager-owned subscription; DONE + re-IDLE heartbeat; poll fallback intact |
| IMAP server (UTFPR) capabilities | Assuming IDLE/CONDSTORE/QRESYNC/ManageSieve exist | Live CAPABILITY probe + 4190 probe recorded before planning each protocol phase |
| CONDSTORE `ENABLE` + MODSEQ FETCH | Untagged MODSEQ breaks existing FETCH parsers (NAMESPACE-class bug) | Transcript tripwire test with MODSEQ/VANISHED responses before enabling |
| QRESYNC VANISHED (vs EXPUNGE) | Treating VANISHED UIDs as EXPUNGE without epoch check | Route through existing tombstoning/convergence machinery with UIDVALIDITY gate first |
| ManageSieve RFC 5804 (port 4190) | Building uploader before knowing server speaks it | Probe → preview-text → upload-behind-confirm; `fileinto`-only subset |
| Zimbra-style servers (possible at UTFPR) | ManageSieve absent; filters live behind SOAP/REST | Probe decides: no 4190 → explicit deferral, local-rules-only |
| OS keyring + `.live-test.env` | Creds echoed in logs, committed env file | Scrubber + `git check-ignore` pre-flight + post-run secret grep |
| SMTP 587 send under retry tests | Double delivery on retry | Message-ID dedupe + Sent-filing asserted exactly-once through queue |
| imap-proto 0.16 parser | New untagged responses (MODSEQ, VANISHED, METADATA) poison session | Fake-transcript regression tests for every newly-enabled extension |

## Performance Traps

| Trap | Symptoms | Prevention | When It Breaks |
|------|----------|------------|----------------|
| IDLE wakeup per EXISTS → full sync each | CPU/battery burn on busy lists; sync never quiesces | Coalesce wakeups to a dirty flag; one sync per burst; debounce ~seconds | Mailing-list-heavy accounts immediately |
| CONDSTORE skipped → full flag FETCH per sync | Sync latency grows with mailbox size after "upgrade" | Assert fast-path hit rate in tests (fake server: flag change → only CHANGEDSINCE FETCH) | 10k+ message folders |
| Override history unbounded growth | SQLite bloat; digest query slows | Cap + prune policy (e.g. keep N per category / M days); indexed (category, ts) | Months of auto-filing |
| Batch dry-run materializing full UID sets in memory | Memory spike on large folders | Stream/page enumeration; re-resolve at confirm time | Large UTFPR mailboxes on first batch |
| Learner retraining on every override | UI jank; redundant writes | Debounced/incremental update; batch retrain on idle | From the first active user |
| Sieve script regenerated per rule edit + auto-uploaded | Server thrash; version conflicts | Explicit export action only; never auto-sync; show diff first | Any active rule editor |

## Security Mistakes

| Mistake | Risk | Prevention |
|---------|------|------------|
| `LOGIN` creds in persisted transcripts/logs | Real mailbox password compromise | Transcript scrubber + post-run secret grep (Pitfall 4) |
| `.live-test.env` committed | Credential leak to git history/remote | `git check-ignore` pre-flight assertion; env path documented git-ignored |
| Imported rule patterns as unvalidated regex/SEARCH | ReDoS on matcher; server command injection via SEARCH fragments | Compile-check + length/timeout caps; parameterized SEARCH only |
| Sieve upload activating `vacation`/`reject` | Mail loss / backscatter from a misclick | `fileinto`-only subset; no vacation/reject UI ever in v1.5 |
| Auto-confirm exfiltrating via server script | Active Sieve script files sensitive mail where local redaction assumed it | Coexistence badges; sensitive-data rule applies to exported scripts (no secret-bearing examples) |
| Quoted real-mail bodies in reply-test logs | Sensitive content in CI/test artifacts | Synthetic fixtures only for compose tests |

## UX Pitfalls

| Pitfall | User Impact | Better Approach |
|---------|-------------|-----------------|
| Silent auto-filing with no digest | "Mail disappears"; trust collapse | Weekly digest + one-click undo + precision display (Pitfall 3) |
| Auto-confirm toggle without precision context | User enables on a 60%-accurate category | Show confirms/overrides at the toggle; require N-streak to enable |
| Sieve/local double-filing flapping | Same mail bounces between folders | Mutual exclusion + "managed by Sieve" badges (Pitfall 5) |
| Dry-run preview going stale silently | User confirms action on wrong messages | Re-resolve + show delta at confirm time ("2 arrived since preview") |
| Retry-failed with no per-item reason | User spams retry on a poison row | Per-row failure reason + targeted retry; poison rows quarantined |
| IDLE status shown as "live" when on poll fallback | False sense of push; confusion on delays | Honest connection state: Push / Polling (15s…/5-min) / Offline |

## "Looks Done But Isn't" Checklist

- [ ] **IDLE:** Often missing DONE-on-all-exits + 29-min re-IDLE + poll fallback — verify by killing the connection mid-IDLE and asserting reconnect with no duplicate sync
- [ ] **CONDSTORE:** Often missing UIDVALIDITY-first gating of stored MODSEQ — verify with a fake UIDVALIDITY bump and assert MODSEQ discarded + full resync
- [ ] **Auto-confirm:** Often missing auto-disengage on override + digest/undo — verify override flips the category back to manual
- [ ] **Learner:** Often missing explicit-signal-only training — verify auto-filed-unseen mail is training-neutral
- [ ] **Sieve export:** Often missing the 4190 probe + coexistence story — verify probe transcript recorded; no upload path without it
- [ ] **Rule import:** Often missing round-trip fixpoint test — verify export(import(x)) == x over generated sets
- [ ] **Batch dry-run:** Often missing UID re-resolution at confirm — verify confirm-after-arrival aborts or translates correctly
- [ ] **Retry-failed:** Often missing row-level in-flight claims — verify double-triggered retry executes each row once
- [ ] **Live runs:** Often missing post-run secret grep + cleanup verification — verify no `LOGIN`/password in artifacts and `SGE-Test-*` gone
- [ ] **Compose:** Often missing expanded-recipient allowlist on reply-all — verify fixture with foreign Cc aborts the run
- [ ] **Exactly-once:** Often missing Message-ID dedupe assertion — verify two queued sends yield one delivery

## Recovery Strategies

| Pitfall | Recovery Cost | Recovery Steps |
|---------|--------------|----------------|
| IDLE connection hijack / race | MEDIUM | Revert to poll-only (flag-gate IDLE off); no data model changed, poll is intact — then redo ownership model |
| Stale-MODSEQ misfiled flags | HIGH | UIDVALIDITY-bump full resync heals flags; add epoch-scoping to MODSEQ column; add tripwire test |
| Auto-confirm misfile wave | MEDIUM | Journaled MOVEs reverse via Phase-10 machinery; force all categories to manual; notify via digest |
| Learner poisoned by silence-as-approval | HIGH | Retrain from explicit signals only (override history is the clean dataset); discard auto-filed-unseen positives |
| Secret leaked in artifact | HIGH | Rotate credential immediately; scrub history (`git filter-repo` if committed); add scrubber + grep gate |
| Sieve/local filing fight | MEDIUM | Deactivate server script (or pause local auto-file); reconcile by journal; add mutual-exclusion badges |
| Duplicate replay (double STORE/MOVE) | MEDIUM | Idempotent verbs heal (Seen re-STORE harmless; MOVE failures need per-UID reconcile); add row-level claims |
| Import drift / dangling rules | LOW | Re-export from source; quarantine rules with missing folders; add round-trip property test |
| Duplicate live send | LOW–MEDIUM | Apology mail if external (sandbox should prevent); assert Message-ID dedupe; keep sends own-address-only |

## Pitfall-to-Phase Mapping

| Pitfall | Prevention Phase | Verification |
|---------|------------------|--------------|
| 1 — IDLE connection ownership | IDLE phase (first protocol phase); design record before code | Mid-IDLE kill test: reconnect, no duplicate sync, poll fallback green |
| 2 — MODSEQ/UIDVALIDITY interplay | CONDSTORE phase (after IDLE); live capability probe as entry gate | Fake UIDVALIDITY-bump discards MODSEQ; transcript tripwire parses VANISHED |
| 6 — replay races from push triggers | IDLE phase (SyncGate funnel) + batch/retry phase (row claims) | Burst-EXISTS + double-click-retry stress: each outbox row executes once |
| 4 — secret leakage in live runs | Live-validation phase; scrubber + pre-flight as entry criteria | Post-run grep for `LOGIN`/password clean; `git check-ignore` asserted |
| 8 — compose sandbox violations | Compose-hardening phase | Foreign-Cc fixture aborts; exactly-once + cleanup assertions green |
| 3 — auto-confirm trust erosion | Classifier follow-through (history → auto-confirm → learner order) | Override auto-disengages; digest + undo exercised |
| 7 — rule import/export drift | Classifier follow-through | Round-trip property test; unknown-version fails closed |
| 5 — Sieve second-product trap | Late dedicated Sieve phase; 4190 probe as entry gate | Probe transcript on file; preview-text ships before any upload path |

Suggested v1.5 phase order following from the above: **live-validation first**
(proves server capabilities — IDLE/CONDSTORE/4190 probes — that gate every
protocol decision), **IDLE second** (ownership model + trigger funnel while poll
stays fallback), **CONDSTORE third** (depends on stable IDLE-era SELECT
behavior), **classifier follow-through fourth** (history → auto-confirm →
learner, with trust governors), **batch dry-run + retry-failed fifth** (needs
the trigger funnel + row claims), **compose hardening sixth** (sandbox already
proven, pipelines exercised throughout), **Sieve last and conditional**
(probe-gated; converts to export-as-text on probe failure).

## Sources

- Codebase: `src-tauri/src/imap/manager.rs` + `imap/session.rs` (single-session,
  lease pattern, capability tests incl. `STARTTLS IDLE` transcripts),
  `src-tauri/src/sync/` (SyncGate single-flight, outbox replay, convergence),
  `src-tauri/src/store/mod.rs` (RFC 4549 drop rules, UIDVALIDITY epoch hygiene),
  `src-tauri/src/sync/bodies.rs` (BODY.PEEK + ammonia sanitize),
  `.planning/PROJECT.md` (v1.5 scope, sandbox contract, constraints) — HIGH
- RFC 2177 (IDLE, 29-min timeout), RFC 4551/7162 (CONDSTORE/QRESYNC, MODSEQ,
  VANISHED, HIGHESTMODSEQ), RFC 5228 (Sieve), RFC 5804 (ManageSieve/4190) —
  protocol knowledge, MEDIUM
- `async-imap` repo (`src/extensions/idle.rs` — IdleHandle exists) and docs.rs;
  `daaki-imap`/`io-imap` (CONDSTORE/QRESYNC live in other crates, not the pinned
  async-imap 0.11 line); ManageSieve client crate search (none surfaced) — LOW
  (websearch provider tier)
- Prior-arounds evidence: imap-proto 0.16 NAMESPACE tripwire, exit-2 SKIP
  contracts, `Auto/` trust gates (single-confirm vs batch-explicit) — HIGH
  (project history)

---
*Pitfalls research for: SGE v1.5 additions (IDLE/CONDSTORE, Sieve export,
auto-confirm policies, live-credential runs on a single-session client)*
*Researched: 2026-10-13*
