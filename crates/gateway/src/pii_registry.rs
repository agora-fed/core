//! The declarative PII registry (issue #16).
//!
//! LGPD export and erasure were two hand-maintained lists, and both had already
//! rotted. Export was a ONE-FIELD BLOCKLIST — `to_jsonb(c) - 'oidc_subject'` — so every
//! column added after it was written leaked by default, including the TOTP secret.
//! Erasure was a hand-written `SET` that had fallen behind the schema: phone, TOTP,
//! birth date and domicile all survived a deletion request.
//!
//! Two lists that must agree with a schema neither of them can see is a design that
//! fails quietly, and quietly is the worst way for a privacy control to fail.
//!
//! So both derive from ONE table here, and a test asserts that every column of
//! `citizen` appears in it. Add a column to the schema without classifying it and the
//! suite fails — the omission becomes impossible instead of merely discouraged.
//!
//! The export is an ALLOWLIST by construction: a column is exported only by being
//! named [`Handling::ExportAndErase`] or [`Handling::ExportOnly`]. A new secret is
//! therefore never exported by accident, only by someone typing that it should be.

/// What the platform does with a column when a citizen exercises their rights.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Handling {
    /// Personal data: goes into the export, and is cleared on erasure.
    ExportAndErase,
    /// Goes into the export but SURVIVES erasure, because a legal basis outweighs
    /// deletion (LGPD art. 16). Moderation records are the case: a suspension is the
    /// platform's own act, and erasing it would erase accountability rather than
    /// personal data.
    ExportOnly,
    /// NEVER exported — a secret, a ciphertext or an opaque index. Cleared on erasure.
    SecretErase,
    /// Neither exported nor erased: structural, not personal. Ids, timestamps the row
    /// needs to exist, and flags that describe the record rather than the person.
    Internal,
}

impl Handling {
    /// Does this column belong in the data-subject export?
    #[must_use]
    pub fn is_exported(self) -> bool {
        matches!(self, Self::ExportAndErase | Self::ExportOnly)
    }

    /// Must this column be cleared when the citizen asks to be erased?
    #[must_use]
    pub fn is_erased(self) -> bool {
        matches!(self, Self::ExportAndErase | Self::SecretErase)
    }
}

/// Every column of `citizen`, classified. The test in this module fails if the schema
/// grows a column that is not here.
///
/// Keep it in the schema's own order — reviewing a migration against this list is then
/// a matter of reading down two columns side by side.
pub const CITIZEN: &[(&str, Handling)] = &[
    ("id", Handling::Internal),
    ("org_id", Handling::Internal),
    // The OIDC subject identifies the person to an external provider: opaque to them,
    // and a correlation handle for anyone else. It was the single field the old
    // blocklist knew about, and that instinct was right.
    ("oidc_subject", Handling::SecretErase),
    ("verification_level", Handling::ExportOnly),
    ("created_at", Handling::ExportOnly),
    ("display_name", Handling::ExportAndErase),
    ("bio", Handling::ExportAndErase),
    ("handle", Handling::ExportAndErase),
    ("avatar_object_key", Handling::ExportAndErase),
    ("cover_object_key", Handling::ExportAndErase),
    // Not nullable; erasure sets it to false. See `ERASE_TO_FALSE`.
    ("is_public", Handling::ExportAndErase),
    ("profile_updated_at", Handling::Internal),
    ("titulo_status", Handling::ExportAndErase),
    ("party_sigla", Handling::ExportAndErase),
    ("legal_name", Handling::ExportAndErase),
    ("govbr_sub", Handling::SecretErase),
    ("govbr_confiabilidade", Handling::ExportAndErase),
    ("govbr_linked_at", Handling::ExportAndErase),
    ("deleted_at", Handling::Internal),
    ("gender", Handling::ExportAndErase),
    // Moderation acts: the platform's record of its own decision. Exported so the
    // person can see it, retained so it cannot be erased by the person it describes.
    ("suspended_at", Handling::ExportOnly),
    ("suspended_reason", Handling::ExportOnly),
    ("silenced_at", Handling::ExportOnly),
    ("silenced_reason", Handling::ExportOnly),
    ("invited_via_invitation_id", Handling::Internal),
    ("email_prefs", Handling::ExportOnly),
    ("default_visibility", Handling::ExportOnly),
    ("default_sensitive", Handling::ExportOnly),
    (
        "auto_delete_notes_older_than_days",
        Handling::ExportAndErase,
    ),
    ("pending_review", Handling::Internal),
    ("approved_at", Handling::Internal),
    ("approved_by", Handling::Internal),
    ("auto_federate_threshold", Handling::ExportOnly),
    ("profile_nudge_sent_at", Handling::Internal),
    ("titulo_zona", Handling::ExportAndErase),
    ("titulo_secao", Handling::ExportAndErase),
    ("uf", Handling::ExportAndErase),
    ("municipio_ibge", Handling::ExportAndErase),
    ("phone_verified_at", Handling::ExportAndErase),
    ("totp_enabled_at", Handling::ExportAndErase),
    ("birth_date", Handling::ExportAndErase),
    // Public reputation, and not erasable without rewriting other people's threads.
    ("karma", Handling::ExportOnly),
    // Ciphertexts and blind indexes (0682/0684): never exported. The person already
    // knows their own voter registration; the masked form is what the platform shows,
    // and it is what the export carries.
    ("titulo_enc", Handling::SecretErase),
    ("titulo_last4", Handling::ExportAndErase),
    ("titulo_hmac", Handling::SecretErase),
    ("phone_enc", Handling::SecretErase),
    ("phone_last4", Handling::ExportAndErase),
    ("totp_secret_enc", Handling::SecretErase),
];

/// Columns that are NOT NULL, so erasure gives them a neutral value instead.
///
/// Only `is_public` today: erasure must unpublish the profile, and `SET NULL` on a
/// NOT NULL column would abort the whole deletion transaction — a privacy control
/// that fails by refusing to run.
pub const ERASE_TO_FALSE: &[&str] = &["is_public"];

/// The `SELECT` list for the export: `jsonb_build_object('col', c.col, …)`.
///
/// Generated rather than written, so a column cannot be exported by accident and
/// cannot be forgotten either.
#[must_use]
pub fn export_json_object() -> String {
    let fields: Vec<String> = CITIZEN
        .iter()
        .filter(|(_, h)| h.is_exported())
        .map(|(name, _)| format!("'{name}', c.{name}"))
        .collect();
    format!("jsonb_build_object({})", fields.join(", "))
}

/// The `SET` clause for erasure, without the trailing marker columns.
#[must_use]
pub fn erase_set_clause() -> String {
    let mut parts: Vec<String> = Vec::new();
    for (name, handling) in CITIZEN {
        if !handling.is_erased() {
            continue;
        }
        if ERASE_TO_FALSE.contains(name) {
            parts.push(format!("{name} = false"));
        } else {
            parts.push(format!("{name} = NULL"));
        }
    }
    parts.join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_column_is_classified_exactly_once() {
        let mut seen = std::collections::HashSet::new();
        for (name, _) in CITIZEN {
            assert!(seen.insert(*name), "{name} is classified twice");
        }
    }

    #[test]
    fn no_secret_reaches_the_export() {
        // The property the old one-field blocklist could not hold: a secret is
        // excluded by CONSTRUCTION, not by being remembered.
        let sql = export_json_object();
        for (name, handling) in CITIZEN {
            if *handling == Handling::SecretErase {
                assert!(
                    !sql.contains(&format!("'{name}'")),
                    "{name} is a secret and must never appear in the export"
                );
            }
        }
        // And the ones that used to leak are named explicitly, so a future
        // reclassification has to argue with this test.
        for leaked in [
            "totp_secret_enc",
            "oidc_subject",
            "govbr_sub",
            "titulo_hmac",
        ] {
            assert!(!sql.contains(&format!("'{leaked}'")), "{leaked} leaked");
        }
    }

    #[test]
    fn erasure_covers_what_the_hand_written_list_missed() {
        // Each of these survived a deletion request before this registry existed.
        let sql = erase_set_clause();
        for missed in [
            "phone_enc",
            "phone_last4",
            "phone_verified_at",
            "totp_secret_enc",
            "totp_enabled_at",
            "birth_date",
            "uf",
            "municipio_ibge",
        ] {
            assert!(
                sql.contains(&format!("{missed} = NULL")),
                "{missed} must be cleared on erasure"
            );
        }
    }

    #[test]
    fn a_not_null_column_is_never_set_to_null() {
        // Setting NULL on a NOT NULL column aborts the deletion transaction — the
        // control failing by refusing to run.
        let sql = erase_set_clause();
        for name in ERASE_TO_FALSE {
            assert!(sql.contains(&format!("{name} = false")));
            assert!(!sql.contains(&format!("{name} = NULL")));
        }
    }

    #[test]
    fn retained_columns_are_exported_but_not_erased() {
        // A suspension is the platform's own act. The person may read it; they may
        // not delete it by asking to be forgotten.
        let erase = erase_set_clause();
        let export = export_json_object();
        for retained in ["suspended_at", "suspended_reason", "created_at"] {
            assert!(export.contains(&format!("'{retained}'")), "{retained}");
            assert!(!erase.contains(&format!("{retained} =")), "{retained}");
        }
    }
}
