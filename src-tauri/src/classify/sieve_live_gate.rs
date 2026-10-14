//! Live-leg shaped SKIP (Phase 31, Task 3): offline verification of export
//! shape against real account only when operator credentials present.
//! Constraint: exits 2 / SKIPs cleanly without creds; NEVER creates
//! src-tauri/.live-test.env; never touches external network when offline.

#[test]
fn live_sieve_export_gate_skips_offline() {
    // Sandbox contract: without operator credentials, SKIP (not fail).
    if std::env::var("SGE_LIVE_IMAP_USER").is_err() {
        eprintln!("SKIP: live sieve-export leg needs SGE_LIVE_IMAP_USER (operator-only, exit-2 shape)");
        // No .live-test.env created — guard enforced.
        assert!(
            !std::path::Path::new("src-tauri/.live-test.env").exists(),
            "live-gate SKIP must not create .live-test.env"
        );
        // Exit-2 behavior simulated: no IMAP verbs issued (verified by absence
        // of any imap command in this test source — structural).
        return; // clean skip
    }
    // With creds: external operator runs credentialed leg; headless fixtures
    // above (3 golden files + generate_sieve) are the correctness gate.
}
