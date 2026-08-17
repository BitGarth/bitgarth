use crate::legal::ValidatedLegalAcknowledgement;
use crate::models::UserId;
use rusqlite::{Transaction, params};
use ulid::Ulid;

pub(crate) fn insert_registration_acceptances(
    tx: &Transaction<'_>,
    user_id: UserId,
    acknowledgement: &ValidatedLegalAcknowledgement,
    accepted_at: &str,
) -> Result<(), rusqlite::Error> {
    insert_acceptance(
        tx,
        user_id,
        "terms",
        &acknowledgement.terms_version,
        accepted_at,
    )?;
    insert_acceptance(
        tx,
        user_id,
        "privacy",
        &acknowledgement.privacy_version,
        accepted_at,
    )?;
    Ok(())
}

fn insert_acceptance(
    tx: &Transaction<'_>,
    user_id: UserId,
    document_kind: &str,
    document_version: &str,
    accepted_at: &str,
) -> Result<(), rusqlite::Error> {
    tx.execute(
        "INSERT INTO legal_acceptances (
            legal_acceptance_id,
            user_id,
            document_kind,
            document_version,
            accepted_at,
            source,
            created_at
        ) VALUES (?1, ?2, ?3, ?4, ?5, 'registration', ?5)",
        params![
            Ulid::new().to_string(),
            user_id.to_string(),
            document_kind,
            document_version,
            accepted_at,
        ],
    )?;
    Ok(())
}
