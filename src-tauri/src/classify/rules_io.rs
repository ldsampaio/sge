//! Rule import/export with versioned JSON schema (Phase 29, IMP-01).
//!
//! Provides versioned JSON serialization for smart-folder rules,
//! validation on import, conflict resolution, and round-trip fixpoint guarantee.

use crate::classify::rules::Rule;
use crate::store::queries;
use crate::store::Store;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use thiserror::Error;
use uuid::Uuid;

/// Versioned JSON schema for rule export (IMP-01).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuleExport {
    pub version: u32,
    pub rules: Vec<RuleExportItem>,
}

/// Single rule export item (IMP-01).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuleExportItem {
    pub rule_id: Uuid,
    pub name: String,
    pub priority: u16,
    pub conditions_json: Value,
    pub dest_kind: DestKind,
    pub destination: String,
    pub scope_json: Value,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Destination kind for rule export.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum DestKind {
    Folder,
    Category,
}

/// Import result with detailed reporting (IMP-01).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportResult {
    pub imported: u32,
    pub updated: u32,
    pub warnings: Vec<String>,
    pub errors: Vec<String>,
}

/// Validation error kinds for user-facing messages.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ValidationError {
    #[error("empty pattern not allowed")]
    EmptyPattern,
    #[error("reserved name: {0}")]
    ReservedName(String),
    #[error("duplicate name: {0}")]
    DuplicateName(String),
    #[error("priority must be between 1 and 999")]
    PriorityRange,
    #[error("invalid dest_kind: {0} (must be 'folder' or 'category')")]
    InvalidDestKind(String),
    #[error("invalid rule_id format: {0}")]
    InvalidRuleId(String),
    #[error("folder not found: {0}")]
    FolderNotFound(String),
}

/// Internal validation result with warnings.
#[derive(Debug)]
struct ValidationResult {
    valid: bool,
    warnings: Vec<String>,
    errors: Vec<String>,
}

/// Versioned JSON envelope for rule export (IMP-01).
#[derive(Debug, Serialize, Deserialize)]
struct RuleExportEnvelope {
    version: u32,
    rules: Vec<RuleExportItem>,
}

const CURRENT_EXPORT_VERSION: u32 = 1;

/// Export all rules as versioned JSON string (IMP-01).
///
/// Serializes with deterministic field order (alphabetical keys) for fixpoint guarantee.
pub fn export_rules(store: &Arc<Mutex<Store>>) -> Result<String, String> {
    let rules = {
        let guard = store.lock().map_err(|e| e.to_string())?;
        queries::export_all_rules(guard.conn()).map_err(|e| e.to_string())?
    };

    let items: Vec<RuleExportItem> = rules
        .into_iter()
        .map(|rule| rule_to_export_item(&rule))
        .collect();

    let envelope = RuleExportEnvelope {
        version: CURRENT_EXPORT_VERSION,
        rules: items,
    };

    // Serialize with sorted keys for deterministic output (fixpoint)
    let mut serializer = serde_json::Serializer::new(Vec::new());
    envelope.serialize(&mut serializer).map_err(|e| e.to_string())?;
    let bytes = serializer.into_inner();
    // Re-serialize with sorted keys using serde_json::to_string_pretty with custom ordering
    // For true alphabetical ordering, we use a custom approach
    let value: Value = serde_json::to_value(&envelope).map_err(|e| e.to_string())?;
    let normalized = normalize_json_keys(&value);
    serde_json::to_string_pretty(&normalized).map_err(|e| e.to_string())
}

/// Normalize JSON keys to alphabetical order for deterministic serialization.
fn normalize_json_keys(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut sorted: serde_json::Map<String, Value> = serde_json::Map::new();
            let mut keys: Vec<_> = map.keys().collect();
            keys.sort();
            for key in keys {
                sorted.insert(key.clone(), normalize_json_keys(map[key].as_ref()));
            }
            Value::Object(sorted)
        }
        Value::Array(arr) => Value::Array(arr.iter().map(normalize_json_keys).collect()),
        other => other.clone(),
    }
}

/// Convert a Rule to RuleExportItem.
fn rule_to_export_item(rule: &Rule) -> RuleExportItem {
    RuleExportItem {
        rule_id: rule.rule_id,
        name: rule.name.clone(),
        priority: rule.priority,
        conditions_json: rule.conditions_json.clone(),
        dest_kind: match rule.dest_kind {
            crate::classify::rules::DestKind::Folder => DestKind::Folder,
            crate::classify::rules::DestKind::Category => DestKind::Category,
        },
        destination: rule.destination.clone(),
        scope_json: rule.scope_json.clone(),
        enabled: rule.enabled,
        created_at: rule.created_at,
        updated_at: rule.updated_at,
    }
}

/// Validate a RuleExportItem and return warnings/errors.
fn validate_rule_item(item: &RuleExportItem, existing_names: &HashSet<String>) -> ValidationResult {
    let mut warnings = Vec::new();
    let mut errors = Vec::new();

    // Empty pattern check
    if item.conditions_json.as_array().map_or(true, |a| a.is_empty()) {
        errors.push("empty pattern not allowed".to_string());
    }

    // Reserved names (case-insensitive)
    let name_lower = item.name.to_lowercase();
    if name_lower == "system" || name_lower == "auto" {
        errors.push(format!("reserved name: {}", item.name));
    }

    // Duplicate name within this import
    if existing_names.contains(&item.name) {
        errors.push(format!("duplicate name: {}", item.name));
    }

    // Priority range
    if item.priority == 0 || item.priority > 999 {
        errors.push("priority must be between 1 and 999".to_string());
    }

    // dest_kind enum check (handled by serde deserialization, but double-check)
    if !matches!(item.dest_kind, DestKind::Folder | DestKind::Category) {
        errors.push(format!("invalid dest_kind: {:?}", item.dest_kind));
    }

    // rule_id format (handled by Uuid deserialization, but double-check)
    // Uuid parsing would fail at deserialization time if invalid

    ValidationResult {
        valid: errors.is_empty(),
        warnings,
        errors,
    }
}

/// Import rules from JSON string (IMP-01).
///
/// Returns ImportResult with imported/updated counts, warnings, and errors.
pub fn import_rules(
    store: &Arc<Mutex<Store>>,
    json: &str,
) -> Result<ImportResult, String> {
    // Parse JSON
    let envelope: RuleExportEnvelope = serde_json::from_str(json).map_err(|e| e.to_string())?;

    if envelope.version != CURRENT_EXPORT_VERSION {
        return Err(format!(
            "unsupported export version {} (expected {})",
            envelope.version, CURRENT_EXPORT_VERSION
        ));
    }

    // Collect existing rule names for duplicate detection
    let existing_names = {
        let guard = store.lock().map_err(|e| e.to_string())?;
        let conn = guard.conn();
        queries::get_all_rule_names(conn)
            .map_err(|e| e.to_string())?
            .into_iter()
            .collect::<HashSet<_>>()
    };

    let mut imported = 0u32;
    let mut updated = 0u32;
    let mut warnings = Vec::new();
    let mut errors = Vec::new();
    let mut seen_names = HashSet::new();

    for item in envelope.rules {
        // Validate item
        let validation = validate_rule_item(&item, &seen_names);
        if !validation.valid {
            errors.extend(validation.errors);
            continue;
        }
        warnings.extend(validation.warnings);
        seen_names.insert(item.name.clone());

        // Check folder existence in conditions (warn only)
        if let Value::Array(arr) = &item.conditions_json {
            for cond in arr {
                if let Some(folder) = cond.get("folder").and_then(|v| v.as_str()) {
                    if let Ok(guard) = store.lock() {
                        let conn = guard.conn();
                        if queries::get_mailbox_id(conn, folder).map_err(|e| e.to_string())?.is_none() {
                            warnings.push(format!("folder not found: {}", folder));
                        }
                    }
                }
            }
        }

        // Upsert by rule_id
        let guard = store.lock().map_err(|e| e.to_string())?;
        let conn = guard.conn();
        let result = queries::upsert_rule_export(conn, &item).map_err(|e| e.to_string())?;

        match result {
            ImportOutcome::Inserted => imported += 1,
            ImportOutcome::Updated => updated += 1,
        }
    }

    Ok(ImportResult {
        imported,
        updated,
        warnings,
        errors,
    })
}

/// Outcome of upsert_rule_export.
#[derive(Debug, PartialEq, Eq)]
enum ImportOutcome {
    Inserted,
    Updated,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;

    fn test_store() -> Arc<Mutex<Store>> {
        Arc::new(Mutex::new(Store::open_in_memory().unwrap()))
    }

    #[test]
    fn round_trip_fixpoint() {
        let store = test_store();

        // Insert a test rule
        {
            let guard = store.lock().unwrap();
            let conn = guard.conn();
            let rule = crate::classify::rules::Rule {
                rule_id: Uuid::new_v4(),
                name: "Test Rule".to_string(),
                priority: 100,
                conditions_json: serde_json::json!([{"folder": "INBOX", "from": "test@example.com"}]),
                dest_kind: crate::classify::rules::DestKind::Folder,
                destination: "TestFolder".to_string(),
                scope_json: serde_json::json!({}),
                enabled: true,
                created_at: chrono::Utc::now(),
                updated_at: chrono::Utc::now(),
                rule_version: 1,
                rule_etag: None,
            };
            crate::store::queries::upsert_rule(&conn, &rule).unwrap();
        }

        // Export
        let json = export_rules(&store).unwrap();

        // Import (should update existing rule_id)
        let result = import_rules(&store, &json).unwrap();
        assert_eq!(result.updated, 1);
        assert_eq!(result.imported, 0);

        // Export again
        let json2 = export_rules(&store).unwrap();

        // Normalize both for comparison (alphabetical key ordering)
        let v1: serde_json::Value = serde_json::from_str(&json).unwrap();
        let v2: serde_json::Value = serde_json::from_str(&json2).unwrap();
        let n1 = normalize_json_keys(&v1);
        let n2 = normalize_json_keys(&v2);
        assert_eq!(n1, n2, "round-trip fixpoint failed: export→import→export must be identical");
    }

    #[test]
    fn validation_empty_pattern() {
        let store = test_store();
        let json = r#"{"version":1,"rules":[{"rule_id":"550e8400-e29b-41d4-a716-446655440000","name":"Test","priority":100,"conditions_json":[],"dest_kind":"folder","destination":"Test","scope_json":{},"enabled":true,"created_at":"2024-01-01T00:00:00Z","updated_at":"2024-01-01T00:00:00Z"}]}"#;
        let result = import_rules(&test_store(), json).unwrap();
        assert!(!result.errors.is_empty());
        assert!(result.errors.iter().any(|e| e.contains("empty pattern not allowed")));
    }

    #[test]
    fn validation_reserved_name_system() {
        let json = r#"{"version":1,"rules":[{"rule_id":"550e8400-e29b-41d4-a716-446655440000","name":"system","priority":100,"conditions_json":[{"folder":"INBOX"}],"dest_kind":"folder","destination":"Test","scope_json":{},"enabled":true,"created_at":"2024-01-01T00:00:00Z","updated_at":"2024-01-01T00:00:00Z"}]}"#;
        let result = import_rules(&test_store(), json).unwrap();
        assert!(!result.errors.is_empty());
        assert!(result.errors.iter().any(|e| e.contains("reserved name")));
    }

    #[test]
    fn validation_reserved_name_auto() {
        let json = r#"{"version":1,"rules":[{"rule_id":"550e8400-e29b-41d4-a716-446655440000","name":"AUTO","priority":100,"conditions_json":[{"folder":"INBOX"}],"dest_kind":"folder","destination":"Test","scope_json":{},"enabled":true,"created_at":"2024-01-01T00:00:00Z","updated_at":"2024-01-01T00:00:00Z"}]}"#;
        let result = import_rules(&test_store(), json).unwrap();
        assert!(!result.errors.is_empty());
        assert!(result.errors.iter().any(|e| e.contains("reserved name")));
    }

    #[test]
    fn validation_duplicate_name() {
        let json = r#"{"version":1,"rules":[{"rule_id":"550e8400-e29b-41d4-a716-446655440000","name":"Test","priority":100,"conditions_json":[{"folder":"INBOX"}],"dest_kind":"folder","destination":"Test","scope_json":{},"enabled":true,"created_at":"2024-01-01T00:00:00Z","updated_at":"2024-01-01T00:00:00Z"},{"rule_id":"550e8400-e29b-41d4-a716-446655440001","name":"Test","priority":200,"conditions_json":[{"folder":"INBOX"}],"dest_kind":"folder","destination":"Test2","scope_json":{},"enabled":true,"created_at":"2024-01-01T00:00:00Z","updated_at":"2024-01-01T00:00:00Z"}]}"#;
        let result = import_rules(&test_store(), json).unwrap();
        assert!(!result.errors.is_empty());
        assert!(result.errors.iter().any(|e| e.contains("duplicate name")));
    }

    #[test]
    fn validation_priority_too_low() {
        let json = r#"{"version":1,"rules":[{"rule_id":"550e8400-e29b-41d4-a716-446655440000","name":"Test","priority":0,"conditions_json":[{"folder":"INBOX"}],"dest_kind":"folder","destination":"Test","scope_json":{},"enabled":true,"created_at":"2024-01-01T00:00:00Z","updated_at":"2024-01-01T00:00:00Z"}]}"#;
        let result = import_rules(&test_store(), json).unwrap();
        assert!(!result.errors.is_empty());
        assert!(result.errors.iter().any(|e| e.contains("priority must be between 1 and 999")));
    }

    #[test]
    fn validation_priority_too_high() {
        let json = r#"{"version":1,"rules":[{"rule_id":"550e8400-e29b-41d4-a716-446655440000","name":"Test","priority":1000,"conditions_json":[{"folder":"INBOX"}],"dest_kind":"folder","destination":"Test","scope_json":{},"enabled":true,"created_at":"2024-01-01T00:00:00Z","updated_at":"2024-01-01T00:00:00Z"}]}"#;
        let result = import_rules(&test_store(), json).unwrap();
        assert!(!result.errors.is_empty());
        assert!(result.errors.iter().any(|e| e.contains("priority must be between 1 and 999")));
    }

    #[test]
    fn validation_invalid_dest_kind() {
        let json = r#"{"version":1,"rules":[{"rule_id":"550e8400-e29b-41d4-a716-446655440000","name":"Test","priority":100,"conditions_json":[{"folder":"INBOX"}],"dest_kind":"invalid","destination":"Test","scope_json":{},"enabled":true,"created_at":"2024-01-01T00:00:00Z","updated_at":"2024-01-01T00:00:00Z"}]}"#;
        let result = import_rules(&test_store(), json).unwrap();
        assert!(!result.errors.is_empty());
        assert!(result.errors.iter().any(|e| e.contains("invalid dest_kind")));
    }

    #[test]
    fn import_existing_rule_id_updates() {
        let store = test_store();

        // Insert initial rule
        let rule_id = Uuid::new_v4();
        {
            let guard = store.lock().unwrap();
            let conn = guard.conn();
            let rule = crate::classify::rules::Rule {
                rule_id,
                name: "Original".to_string(),
                priority: 100,
                conditions_json: serde_json::json!([{"folder": "INBOX"}]),
                dest_kind: crate::classify::rules::DestKind::Folder,
                destination: "OldFolder".to_string(),
                scope_json: serde_json::json!({}),
                enabled: true,
                created_at: chrono::Utc::now(),
                updated_at: chrono::Utc::now(),
                rule_version: 1,
                rule_etag: None,
            };
            crate::store::queries::upsert_rule(&conn, &rule).unwrap();
        }

        // Import with same rule_id but different data
        let json = format!(r#"{{"version":1,"rules":[{{"rule_id":"{}","name":"Updated","priority":200,"conditions_json":[{{"folder":"INBOX"}}],"dest_kind":"folder","destination":"NewFolder","scope_json":{{}},"enabled":true,"created_at":"2024-01-01T00:00:00Z","updated_at":"2024-01-01T00:00:00Z"}}]}}"#, rule_id);
        let result = import_rules(&store, &json).unwrap();
        assert_eq!(result.updated, 1);
        assert_eq!(result.imported, 0);
    }
}