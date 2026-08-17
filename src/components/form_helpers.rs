use crate::backend::ApiErrorEnvelope;
use crate::models::FieldErrors;
use dioxus::prelude::Signal;
use dioxus::prelude::WritableExt;

pub(crate) fn begin_submit(mut submitting: Signal<bool>) -> bool {
    if submitting() {
        return false;
    }
    *submitting.write() = true;
    true
}

pub(crate) fn finish_submit(mut submitting: Signal<bool>) {
    *submitting.write() = false;
}

pub(crate) fn is_form_field_error(error: &ApiErrorEnvelope) -> bool {
    error.is_validation() || error.is_conflict()
}

pub(crate) fn field_errors_or_empty(error: &ApiErrorEnvelope) -> FieldErrors {
    error.field_errors().cloned().unwrap_or_default()
}

pub(crate) fn first_matching_field_error(
    error: &ApiErrorEnvelope,
    fields_in_priority_order: &[&str],
) -> Option<String> {
    fields_in_priority_order
        .iter()
        .find_map(|field| error.first_field_error(field))
        .cloned()
}

pub(crate) fn primary_field_or_message(
    error: &ApiErrorEnvelope,
    fields_in_priority_order: &[&str],
) -> String {
    first_matching_field_error(error, fields_in_priority_order).unwrap_or_else(|| error.to_string())
}

pub(crate) fn primary_field_or_fallback(
    error: &ApiErrorEnvelope,
    fields_in_priority_order: &[&str],
    fallback: &str,
) -> String {
    first_matching_field_error(error, fields_in_priority_order)
        .unwrap_or_else(|| fallback.to_string())
}

#[cfg(all(test, not(bitgarth_db_unit_only)))]
mod tests {
    use super::*;

    #[test]
    fn first_matching_field_error_uses_priority_order() {
        let mut field_errors = FieldErrors::new();
        field_errors.add("wallet_label", "Wallet label conflict".to_string());
        field_errors.add("address", "Address is invalid".to_string());
        let error = ApiErrorEnvelope::validation("Validation failed", field_errors);

        let selected =
            first_matching_field_error(&error, &["extended_pubkey", "address", "wallet_label"]);
        assert_eq!(selected.as_deref(), Some("Address is invalid"));
    }

    #[test]
    fn primary_field_or_message_falls_back_to_envelope_message() {
        let error = ApiErrorEnvelope::validation("General validation error", FieldErrors::new());

        let selected = primary_field_or_message(&error, &["wallet_label", "address"]);
        assert_eq!(selected, "General validation error");
    }

    #[test]
    fn primary_field_or_fallback_uses_default_when_fields_missing() {
        let error = ApiErrorEnvelope::conflict("Conflict", FieldErrors::new());

        let selected = primary_field_or_fallback(
            &error,
            &["mempool_base_url"],
            "Invalid mempool URL override.",
        );
        assert_eq!(selected, "Invalid mempool URL override.");
    }

    #[test]
    fn is_form_field_error_only_for_validation_and_conflict() {
        let validation_error = ApiErrorEnvelope::validation("Validation", FieldErrors::new());
        let conflict_error = ApiErrorEnvelope::conflict("Conflict", FieldErrors::new());
        let unauthorized_error = ApiErrorEnvelope::unauthorized("Unauthorized");

        assert!(is_form_field_error(&validation_error));
        assert!(is_form_field_error(&conflict_error));
        assert!(!is_form_field_error(&unauthorized_error));
    }
}
