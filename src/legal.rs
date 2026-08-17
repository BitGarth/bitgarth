#[cfg(feature = "server")]
use crate::models::FieldErrors;
use serde::{Deserialize, Serialize};

pub(crate) const TERMS_VERSION: &str = "2026-06-25";
pub(crate) const PRIVACY_VERSION: &str = "2026-05-18";
pub(crate) const TERMS_URL: &str = "https://bitgarth.app/terms.html";
pub(crate) const PRIVACY_URL: &str = "https://bitgarth.app/privacy.html";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct LegalAcknowledgement {
    pub(crate) accepted_terms_version: String,
    pub(crate) accepted_privacy_version: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg(feature = "server")]
pub(crate) struct ValidatedLegalAcknowledgement {
    pub(crate) terms_version: String,
    pub(crate) privacy_version: String,
}

pub(crate) fn current_registration_acknowledgement() -> LegalAcknowledgement {
    LegalAcknowledgement {
        accepted_terms_version: TERMS_VERSION.to_string(),
        accepted_privacy_version: PRIVACY_VERSION.to_string(),
    }
}

#[cfg(feature = "server")]
pub(crate) fn validate_registration_acknowledgement(
    acknowledgement: Option<LegalAcknowledgement>,
) -> Result<ValidatedLegalAcknowledgement, FieldErrors> {
    let mut errors = FieldErrors::new();

    let Some(acknowledgement) = acknowledgement else {
        errors.add(
            "legal_acknowledgement",
            "You must agree to the Terms and acknowledge the Privacy Notice.".to_string(),
        );
        return Err(errors);
    };

    if acknowledgement.accepted_terms_version != TERMS_VERSION {
        errors.add(
            "legal_acknowledgement",
            "You must accept the current Terms.".to_string(),
        );
    }

    if acknowledgement.accepted_privacy_version != PRIVACY_VERSION {
        errors.add(
            "legal_acknowledgement",
            "You must acknowledge the current Privacy Notice.".to_string(),
        );
    }

    if !errors.is_empty() {
        return Err(errors);
    }

    Ok(ValidatedLegalAcknowledgement {
        terms_version: acknowledgement.accepted_terms_version,
        privacy_version: acknowledgement.accepted_privacy_version,
    })
}

#[cfg(all(test, feature = "server", not(bitgarth_db_unit_only)))]
mod tests {
    use super::*;

    #[test]
    fn validate_registration_acknowledgement_accepts_current_versions() {
        let acknowledgement = current_registration_acknowledgement();

        let validated = validate_registration_acknowledgement(Some(acknowledgement))
            .expect("current acknowledgement should validate");

        assert_eq!(validated.terms_version, TERMS_VERSION);
        assert_eq!(validated.privacy_version, PRIVACY_VERSION);
    }

    #[test]
    fn validate_registration_acknowledgement_rejects_missing_acknowledgement() {
        let errors = validate_registration_acknowledgement(None)
            .expect_err("missing acknowledgement should fail");

        assert!(errors.get("legal_acknowledgement").is_some());
    }

    #[test]
    fn validate_registration_acknowledgement_rejects_old_versions() {
        let acknowledgement = LegalAcknowledgement {
            accepted_terms_version: "2026-01-01".to_string(),
            accepted_privacy_version: "2026-01-01".to_string(),
        };

        let errors = validate_registration_acknowledgement(Some(acknowledgement))
            .expect_err("old versions should fail");

        let messages = errors
            .get("legal_acknowledgement")
            .expect("legal acknowledgement errors should exist");
        assert_eq!(messages.len(), 2);
    }
}
