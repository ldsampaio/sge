// SGE default taxonomy module (Phase 16).
//
// The shipped UTFPR pt-BR category system: embedded via include_str! (always
// offline), versioned, ID-stable. Validation rejects everything that would
// orphan labels or break the classifier contract (cycles, dup IDs,
// Auto-root collision, flat-choice budget overflow).

pub mod bridge;
pub mod evidence;
pub mod redact;
pub mod suggest;
pub mod taxonomy;
pub mod worker;
