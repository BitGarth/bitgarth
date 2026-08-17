const HLEDGER_OWNER_DIRECTORY_FALLBACK: &str = "me";
const HLEDGER_OWNER_POSTING_FALLBACK: &str = "Me";

pub(crate) fn hledger_owner_segments_from_username(raw_username: &str) -> (String, String) {
    let owner_directory_segment = normalize_owner_directory_segment(raw_username);
    let owner_posting_segment = normalize_owner_posting_segment(&owner_directory_segment);
    (owner_directory_segment, owner_posting_segment)
}

pub(crate) fn normalize_owner_directory_segment(raw_username: &str) -> String {
    let normalized: String = raw_username
        .chars()
        .filter(|character| !character.is_whitespace())
        .filter(|character| is_hledger_owner_directory_char_safe(*character))
        .collect();

    if normalized.is_empty() || matches!(normalized.as_str(), "." | "..") {
        HLEDGER_OWNER_DIRECTORY_FALLBACK.to_string()
    } else {
        normalized
    }
}

pub(crate) fn normalize_owner_posting_segment(owner_directory_segment: &str) -> String {
    let mut normalized = String::new();
    for word in owner_directory_segment
        .split(|character: char| !character.is_ascii_alphanumeric())
        .filter(|word| !word.is_empty())
    {
        let lowered = word.to_ascii_lowercase();
        let mut chars = lowered.chars();
        if let Some(first_char) = chars.next() {
            normalized.push(first_char.to_ascii_uppercase());
            normalized.push_str(chars.as_str());
        }
    }

    if normalized.is_empty() {
        HLEDGER_OWNER_POSTING_FALLBACK.to_string()
    } else {
        normalized
    }
}

fn is_hledger_owner_directory_char_safe(character: char) -> bool {
    character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
}
