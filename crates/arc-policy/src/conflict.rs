//! Deterministic conflict aggregation for mapping-style policy outputs.

use std::collections::{BTreeMap, BTreeSet};

/// A single source-to-destination mapping candidate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MappingRecord {
    /// Source identifier that produced this mapping.
    pub source: String,
    /// Optional destination key this mapping wants to write.
    pub destination: Option<String>,
    /// Stable rule index for traceability.
    pub rule_index: usize,
}

/// Validation issue types discovered while checking mapping candidates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValidationIssue {
    /// Multiple unique sources write the same destination.
    Conflict {
        /// Conflicting destination key.
        destination: String,
        /// Sources that attempted to write `destination` in deterministic order.
        sources: Vec<String>,
        /// Rule indices corresponding to `sources`.
        rule_indexes: Vec<usize>,
    },
}

impl ValidationIssue {
    /// Destination involved in this issue.
    pub fn destination(&self) -> &str {
        match self {
            ValidationIssue::Conflict { destination, .. } => destination,
        }
    }
}

/// Structured fix applied while validating non-conflicting mappings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValidationFix {
    /// Mapping removed because destination failed policy validation.
    InvalidDestinationDropped {
        /// Destination that was dropped.
        destination: String,
        /// Rule index of the dropped mapping.
        rule_index: usize,
    },
}

/// Validation error with deterministic issue ordering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationError {
    /// All discovered issues.
    pub issues: Vec<ValidationIssue>,
}

impl std::fmt::Display for ValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Found {} mapping issue(s): {}",
            self.issues.len(),
            self.issues
                .iter()
                .map(|issue| issue.destination().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        )
    }
}

impl std::error::Error for ValidationError {}

/// Successful validation output with accepted mappings and applied fixes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedMappings {
    /// Mappings accepted by validation.
    pub accepted: Vec<MappingRecord>,
    /// Non-fatal fixes applied during validation.
    pub fixes: Vec<ValidationFix>,
}

/// Validate mappings using the default destination rule (`destination` must be non-empty).
pub fn validate_mappings(
    records: Vec<MappingRecord>,
) -> Result<ValidatedMappings, ValidationError> {
    validate_mappings_with(records, |destination| !destination.is_empty())
}

/// Validate mappings with a caller-provided destination validator.
pub fn validate_mappings_with(
    records: Vec<MappingRecord>,
    is_valid_destination: impl Fn(&str) -> bool,
) -> Result<ValidatedMappings, ValidationError> {
    let mut accepted = Vec::with_capacity(records.len());
    let mut fixes = Vec::new();
    for record in records {
        match &record.destination {
            Some(destination) if !is_valid_destination(destination) => {
                fixes.push(ValidationFix::InvalidDestinationDropped {
                    destination: destination.clone(),
                    rule_index: record.rule_index,
                });
            }
            _ => accepted.push(record),
        }
    }

    let mut by_destination: BTreeMap<String, BTreeMap<String, BTreeSet<usize>>> = BTreeMap::new();
    for record in &accepted {
        if let Some(destination) = &record.destination {
            by_destination
                .entry(destination.clone())
                .or_default()
                .entry(record.source.clone())
                .or_default()
                .insert(record.rule_index);
        }
    }

    let mut issues = Vec::new();
    for (destination, contributors) in by_destination {
        if contributors.len() > 1 {
            let mut sources = Vec::with_capacity(contributors.len());
            let mut rule_indexes = Vec::with_capacity(contributors.len());
            for (source, source_rules) in contributors {
                sources.push(source);
                rule_indexes.push(*source_rules.first().expect("rules are non-empty"));
            }
            issues.push(ValidationIssue::Conflict { destination, sources, rule_indexes });
        }
    }

    if !issues.is_empty() {
        return Err(ValidationError { issues });
    }

    Ok(ValidatedMappings { accepted, fixes })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_conflicts_in_deterministic_order() {
        let records = vec![
            MappingRecord {
                source: "source-b".to_string(),
                destination: Some("alpha".to_string()),
                rule_index: 2,
            },
            MappingRecord {
                source: "source-a".to_string(),
                destination: Some("alpha".to_string()),
                rule_index: 1,
            },
        ];

        let error = validate_mappings(records).expect_err("conflict expected");
        assert_eq!(error.issues.len(), 1);
        assert_eq!(
            error.issues[0],
            ValidationIssue::Conflict {
                destination: "alpha".to_string(),
                sources: vec!["source-a".to_string(), "source-b".to_string()],
                rule_indexes: vec![1, 2],
            }
        );
    }

    #[test]
    fn drops_invalid_destinations_as_structured_fix() {
        let records = vec![
            MappingRecord {
                source: "source-a".to_string(),
                destination: Some("".to_string()),
                rule_index: 1,
            },
            MappingRecord {
                source: "source-b".to_string(),
                destination: Some("ok".to_string()),
                rule_index: 2,
            },
        ];

        let out = validate_mappings(records).expect("validation should pass with fix");
        assert_eq!(out.accepted.len(), 1);
        assert_eq!(
            out.fixes,
            vec![ValidationFix::InvalidDestinationDropped {
                destination: "".to_string(),
                rule_index: 1,
            }]
        );
    }

    #[test]
    fn accepts_records_without_destination() {
        let records = vec![MappingRecord {
            source: "source-a".to_string(),
            destination: None,
            rule_index: 1,
        }];

        let out = validate_mappings(records).expect("validation should pass");
        assert_eq!(out.accepted.len(), 1);
        assert!(out.fixes.is_empty());
    }

    #[test]
    fn duplicate_source_rules_do_not_trigger_multi_source_conflict() {
        let records = vec![
            MappingRecord {
                source: "source-a".to_string(),
                destination: Some("alpha".to_string()),
                rule_index: 1,
            },
            MappingRecord {
                source: "source-a".to_string(),
                destination: Some("alpha".to_string()),
                rule_index: 2,
            },
        ];

        let out = validate_mappings(records).expect("same source should not conflict");
        assert_eq!(out.accepted.len(), 2);
        assert!(out.fixes.is_empty());
    }

    #[test]
    fn invalid_destinations_are_dropped_before_conflict_detection() {
        let records = vec![
            MappingRecord {
                source: "source-a".to_string(),
                destination: Some("".to_string()),
                rule_index: 1,
            },
            MappingRecord {
                source: "source-b".to_string(),
                destination: Some("".to_string()),
                rule_index: 2,
            },
        ];

        let out = validate_mappings(records)
            .expect("invalid destinations should be fixed, not conflicted");
        assert!(out.accepted.is_empty());
        assert_eq!(out.fixes.len(), 2);
    }
}
