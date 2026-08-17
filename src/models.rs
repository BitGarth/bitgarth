use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};
#[cfg(feature = "server")]
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fmt;
use std::str::FromStr;
use ulid::Ulid;
use url::Url;

// ============ Validation Constants ============

/// Minimum password length
pub(crate) const PASSWORD_MIN_LENGTH: usize = 8;
/// Maximum username length
pub(crate) const USERNAME_MAX_LENGTH: usize = 64;

// ============ ULID Identifiers ============

#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct UserId(Ulid);

impl fmt::Debug for UserId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("UserId").field(&self.0.to_string()).finish()
    }
}

impl UserId {
    pub fn new() -> Self {
        UserId(Ulid::new())
    }

    pub fn as_ulid(&self) -> Ulid {
        self.0
    }
}

impl fmt::Display for UserId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for UserId {
    type Err = ulid::DecodeError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ulid::from_string(s).map(UserId)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
#[cfg(feature = "server")]
pub(crate) struct SessionId(Ulid);

#[cfg(feature = "server")]
impl fmt::Debug for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("SessionId")
            .field(&self.0.to_string())
            .finish()
    }
}

#[cfg(feature = "server")]
impl SessionId {
    pub(crate) fn new() -> Self {
        SessionId(Ulid::new())
    }
}

#[cfg(feature = "server")]
impl fmt::Display for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[cfg(feature = "server")]
impl FromStr for SessionId {
    type Err = ulid::DecodeError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ulid::from_string(s).map(SessionId)
    }
}

#[cfg(feature = "server")]
#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub(crate) struct CredentialId(Ulid);

#[cfg(feature = "server")]
impl fmt::Debug for CredentialId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("CredentialId")
            .field(&self.0.to_string())
            .finish()
    }
}

#[cfg(feature = "server")]
impl CredentialId {
    pub(crate) fn new() -> Self {
        CredentialId(Ulid::new())
    }
}

#[cfg(feature = "server")]
impl fmt::Display for CredentialId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[cfg(feature = "server")]
impl FromStr for CredentialId {
    type Err = ulid::DecodeError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ulid::from_string(s).map(CredentialId)
    }
}

// ============ Field Validation Errors ============

/// A collection of validation errors keyed by field name.
/// Used to communicate validation failures from backend to frontend.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub(crate) struct FieldErrors(pub HashMap<String, Vec<String>>);

impl FieldErrors {
    pub(crate) fn new() -> Self {
        Self(HashMap::new())
    }

    pub(crate) fn add(&mut self, field: &str, message: String) {
        self.0.entry(field.to_string()).or_default().push(message);
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub(crate) fn get(&self, field: &str) -> Option<&Vec<String>> {
        self.0.get(field)
    }

    pub(crate) fn first(&self, field: &str) -> Option<&String> {
        self.0.get(field).and_then(|v| v.first())
    }
}

impl fmt::Display for FieldErrors {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let messages: Vec<String> = self
            .0
            .iter()
            .flat_map(|(field, errors)| errors.iter().map(move |e| format!("{}: {}", field, e)))
            .collect();
        write!(f, "{}", messages.join("; "))
    }
}

impl std::error::Error for FieldErrors {}

// ============ Username ============

/// Raw username as received from user input. No validation performed.
/// Use for form submission to backend.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RawUsername(String);

impl RawUsername {
    pub fn new(s: String) -> Self {
        RawUsername(s)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Validate and convert to ValidatedUsername
    pub fn validate(self) -> Result<ValidatedUsername, UsernameError> {
        ValidatedUsername::from_raw(self)
    }
}

impl fmt::Display for RawUsername {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A validated username. Can only be constructed through validation.
/// An email address is considered a valid username.
/// Deserialization should NOT be allowed directly; use RawUsername for input, and validate to get this type.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(transparent)]
pub struct ValidatedUsername(String);

#[derive(Debug, Clone, PartialEq)]
pub enum UsernameError {
    Empty,
    TooLong,
    InvalidCharacters,
}

impl fmt::Display for UsernameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            UsernameError::Empty => write!(f, "Username cannot be empty"),
            UsernameError::TooLong => write!(
                f,
                "Username must be {} characters or less",
                USERNAME_MAX_LENGTH
            ),
            UsernameError::InvalidCharacters => {
                write!(
                    f,
                    "Username can only contain letters, numbers, underscores, hyphens, @, and ."
                )
            }
        }
    }
}

impl std::error::Error for UsernameError {}

impl ValidatedUsername {
    /// Create a ValidatedUsername from a RawUsername by validating it.
    /// An email address is considered a valid username.
    pub fn from_raw(raw: RawUsername) -> Result<Self, UsernameError> {
        let s = raw.0;
        if s.is_empty() {
            return Err(UsernameError::Empty);
        }
        if s.len() > USERNAME_MAX_LENGTH {
            return Err(UsernameError::TooLong);
        }
        // Allow alphanumeric, underscore, hyphen, @, and . (for email addresses)
        if !s
            .chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '-' || c == '@' || c == '.')
        {
            return Err(UsernameError::InvalidCharacters);
        }
        Ok(ValidatedUsername(s))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ValidatedUsername {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<ValidatedUsername> for String {
    fn from(u: ValidatedUsername) -> String {
        u.0
    }
}

// ============ Password ============

/// Raw plaintext password as received from user input. No validation performed.
/// Use for form submission to backend.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(transparent)]
pub(crate) struct RawPlaintextPassword(String);

impl RawPlaintextPassword {
    pub(crate) fn new(s: String) -> Self {
        RawPlaintextPassword(s)
    }
}

#[cfg(feature = "server")]
impl RawPlaintextPassword {
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }

    /// Validate and collect ALL errors (for displaying multiple errors to user)
    pub(crate) fn validate_all(
        self,
    ) -> Result<ValidatedPlaintextPassword, Vec<PasswordValidationError>> {
        ValidatedPlaintextPassword::from_raw_all(self)
    }
}

/// A validated plaintext password. Can only be constructed through validation.
/// Validation ensures password meets strength requirements.
/// Should not be serialized or deserialized. Only hashed password are serialized when stored.
#[cfg(feature = "server")]
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ValidatedPlaintextPassword(String);

#[cfg(feature = "server")]
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum PasswordValidationError {
    TooShort,
    MissingUppercase,
    MissingLowercase,
    MissingNumber,
}

#[cfg(feature = "server")]
impl fmt::Display for PasswordValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PasswordValidationError::TooShort => {
                write!(
                    f,
                    "Password must be at least {} characters",
                    PASSWORD_MIN_LENGTH
                )
            }
            PasswordValidationError::MissingUppercase => {
                write!(f, "Password must contain at least one uppercase letter")
            }
            PasswordValidationError::MissingLowercase => {
                write!(f, "Password must contain at least one lowercase letter")
            }
            PasswordValidationError::MissingNumber => {
                write!(f, "Password must contain at least one number")
            }
        }
    }
}

#[cfg(feature = "server")]
impl std::error::Error for PasswordValidationError {}

#[cfg(feature = "server")]
impl ValidatedPlaintextPassword {
    /// Create a ValidatedPlaintextPassword, collecting ALL validation errors.
    /// Use this when you want to show all problems to the user at once.
    pub(crate) fn from_raw_all(
        raw: RawPlaintextPassword,
    ) -> Result<Self, Vec<PasswordValidationError>> {
        let s = &raw.0;
        let mut errors = Vec::new();

        if s.len() < PASSWORD_MIN_LENGTH {
            errors.push(PasswordValidationError::TooShort);
        }
        if !s.chars().any(|c| c.is_uppercase()) {
            errors.push(PasswordValidationError::MissingUppercase);
        }
        if !s.chars().any(|c| c.is_lowercase()) {
            errors.push(PasswordValidationError::MissingLowercase);
        }
        if !s.chars().any(|c| c.is_numeric()) {
            errors.push(PasswordValidationError::MissingNumber);
        }

        if errors.is_empty() {
            Ok(ValidatedPlaintextPassword(raw.0))
        } else {
            Err(errors)
        }
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

// ============ SessionToken ============

/// A session token. Wraps a base64-encoded string.
/// Serializes/deserializes as a plain string in JSON.
#[cfg(any(feature = "server", feature = "desktop"))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub(crate) struct SessionToken(String);

#[cfg(feature = "server")]
impl SessionToken {
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

#[cfg(any(feature = "server", feature = "desktop"))]
impl SessionToken {
    /// Create a new SessionToken from a raw base64 string.
    /// This is called internally by session::generate_session_token.
    pub(crate) fn from_raw(s: String) -> Self {
        SessionToken(s)
    }
}

#[cfg(any(feature = "server", feature = "desktop"))]
impl fmt::Display for SessionToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

// ============ TokenHash ============

/// SHA-256 hash of a session token, stored in the database instead of the raw token.
#[cfg(feature = "server")]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct TokenHash(String);

#[cfg(feature = "server")]
impl TokenHash {
    pub(crate) fn from_token(token: &SessionToken) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(token.as_str().as_bytes());
        let hash = hasher.finalize();
        TokenHash(hex::encode(hash))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

#[cfg(feature = "server")]
impl fmt::Display for TokenHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

// ============ DateTime Helper ============

/// Error type for DateTime parsing from database strings
#[cfg(feature = "server")]
#[derive(Debug, Clone)]
pub(crate) struct DateTimeParseError(pub String);

#[cfg(feature = "server")]
impl fmt::Display for DateTimeParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Failed to parse datetime: {}", self.0)
    }
}

#[cfg(feature = "server")]
impl std::error::Error for DateTimeParseError {}

/// Pure function: Parse DateTime from database string.
/// This safely handles parsing errors without panicking.
///
/// Supports both RFC3339 format (e.g., "2026-01-27T15:37:22Z") and
/// SQLite's datetime format (e.g., "2026-01-27 15:37:22").
#[cfg(feature = "server")]
pub(crate) fn parse_datetime(s: &str) -> Result<DateTime<Utc>, DateTimeParseError> {
    // First try RFC3339 format
    if let Ok(dt) = s.parse::<DateTime<Utc>>() {
        return Ok(dt);
    }

    // Try SQLite's default datetime format: "YYYY-MM-DD HH:MM:SS"
    use chrono::NaiveDateTime;
    if let Ok(naive) = NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S") {
        return Ok(naive.and_utc());
    }

    Err(DateTimeParseError(format!(
        "{}: not a valid RFC3339 or SQLite datetime format",
        s
    )))
}

// ============ Core Types ============

/// Core user type
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct User {
    pub user_id: UserId,
    pub username: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Auth method enum for multi-method authentication support
#[expect(
    dead_code,
    reason = "Reserved for future multi-method authentication support"
)]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(crate) enum AuthMethod {
    Password,
    HardwareWallet,
    Passkey,
}

/// Session with typed token
#[cfg(feature = "server")]
#[derive(Debug, Clone)]
pub(crate) struct Session {
    pub session_id: SessionId,
    pub user_id: UserId,
    pub token: SessionToken,
    #[expect(
        dead_code,
        reason = "Retained for future session metadata auditing and diagnostics"
    )]
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

// ============ User Settings ============

use crate::i18n::Locale;

/// Date/time display format options.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DateTimeFormat {
    YearMonthDay24,
    DayMonthYear24,
    MonthDayYear12,
}

impl DateTimeFormat {
    pub fn code(&self) -> &'static str {
        match self {
            DateTimeFormat::YearMonthDay24 => "ymd_24",
            DateTimeFormat::DayMonthYear24 => "dmy_24",
            DateTimeFormat::MonthDayYear12 => "mdy_12",
        }
    }

    pub fn from_code(code: &str) -> Option<Self> {
        match code {
            "ymd_24" => Some(DateTimeFormat::YearMonthDay24),
            "dmy_24" => Some(DateTimeFormat::DayMonthYear24),
            "mdy_12" => Some(DateTimeFormat::MonthDayYear12),
            _ => None,
        }
    }

    pub fn chrono_format(&self) -> &'static str {
        match self {
            DateTimeFormat::YearMonthDay24 => "%Y-%m-%d %H:%M",
            DateTimeFormat::DayMonthYear24 => "%d/%m/%Y %H:%M",
            DateTimeFormat::MonthDayYear12 => "%b %d, %Y %I:%M %p",
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            DateTimeFormat::YearMonthDay24 => "YYYY-MM-DD 24h",
            DateTimeFormat::DayMonthYear24 => "DD/MM/YYYY 24h",
            DateTimeFormat::MonthDayYear12 => "Mon DD, YYYY 12h",
        }
    }
}

/// Numeric formatting options.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NumberFormat {
    DotComma,
    CommaDot,
    CommaSpace,
}

impl NumberFormat {
    pub fn code(&self) -> &'static str {
        match self {
            NumberFormat::DotComma => "dot_comma",
            NumberFormat::CommaDot => "comma_dot",
            NumberFormat::CommaSpace => "comma_space",
        }
    }

    pub fn from_code(code: &str) -> Option<Self> {
        match code {
            "dot_comma" => Some(NumberFormat::DotComma),
            "comma_dot" => Some(NumberFormat::CommaDot),
            "comma_space" => Some(NumberFormat::CommaSpace),
            _ => None,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            NumberFormat::DotComma => "1,234.56",
            NumberFormat::CommaDot => "1.234,56",
            NumberFormat::CommaSpace => "1 234,56",
        }
    }
}

/// Currency code wrapper.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CurrencyCode(pub iso4217_static::Currency);

impl CurrencyCode {
    pub fn code(&self) -> &str {
        self.0.as_ref()
    }

    pub fn symbol(&self) -> &'static str {
        match self.code() {
            "USD" => "$",
            "EUR" => "€",
            "GBP" => "£",
            "ZAR" => "R",
            "JPY" => "¥",
            "CHF" => "CHF",
            "AUD" => "A$",
            "CAD" => "C$",
            _ => "?",
        }
    }

    pub fn label(&self) -> &'static str {
        match self.code() {
            "USD" => "USD ($) — US Dollar",
            "EUR" => "EUR (€) — Euro",
            "GBP" => "GBP (£) — British Pound",
            "ZAR" => "ZAR (R) — South African Rand",
            "JPY" => "JPY (¥) — Japanese Yen",
            "CHF" => "CHF — Swiss Franc",
            "AUD" => "AUD (A$) — Australian Dollar",
            "CAD" => "CAD (C$) — Canadian Dollar",
            _ => "Unknown",
        }
    }

    pub fn from_code(code: &str) -> Option<Self> {
        iso4217_static::Currency::try_from(code)
            .ok()
            .map(CurrencyCode)
    }

    /// Minor-unit digits to show for fiat display. JPY has none; every other
    /// supported currency uses two.
    pub fn decimal_places(&self) -> usize {
        match self.code() {
            "JPY" => 0,
            _ => 2,
        }
    }
}

impl Serialize for CurrencyCode {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.code())
    }
}

impl<'de> Deserialize<'de> for CurrencyCode {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        CurrencyCode::from_code(&value)
            .ok_or_else(|| serde::de::Error::custom(format!("Invalid currency code: {}", value)))
    }
}

/// User timezone wrapper.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct UserTimezone(pub Tz);

impl UserTimezone {
    pub fn name(&self) -> String {
        self.0.to_string()
    }
}

impl Serialize for UserTimezone {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.0.to_string())
    }
}

impl<'de> Deserialize<'de> for UserTimezone {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        value
            .parse::<Tz>()
            .map(UserTimezone)
            .map_err(serde::de::Error::custom)
    }
}

impl From<Tz> for UserTimezone {
    fn from(tz: Tz) -> Self {
        UserTimezone(tz)
    }
}

impl From<UserTimezone> for Tz {
    fn from(tz: UserTimezone) -> Tz {
        tz.0
    }
}

pub(crate) const DEFAULT_MEMPOOL_BASE_URL: &str = "https://mempool.space";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RawMempoolBaseUrl(String);

impl RawMempoolBaseUrl {
    pub fn new(value: String) -> Self {
        Self(value)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

// ============ Etherscan Base URL ============

pub(crate) const DEFAULT_ETHERSCAN_BASE_URL: &str = "https://api.etherscan.io/v2/api";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RawEtherscanBaseUrl(String);

impl RawEtherscanBaseUrl {
    pub fn new(value: String) -> Self {
        Self(value)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

// ============ hledger Account Prefix ============

/// Maximum length of a single hledger account-name segment.
/// Shared source of truth for prefix validation and the export label
/// normalizer in `src/exports/hledger/label.rs`.
pub(crate) const HLEDGER_ACCOUNT_SEGMENT_MAX_LENGTH: usize = 255;

/// A validated hledger account-name prefix, e.g. `assets:Liquid:Crypto`.
///
/// Colon-separated segments; each segment is non-empty, uses only ASCII
/// alphanumerics, `_`, `-`, and single internal spaces, and is at most
/// `HLEDGER_ACCOUNT_SEGMENT_MAX_LENGTH` characters. Serializes as the raw
/// string.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct HledgerAccountPrefix(String);

impl HledgerAccountPrefix {
    pub fn parse(raw: &str) -> Result<Self, HledgerAccountPrefixError> {
        if raw.is_empty() {
            return Err(HledgerAccountPrefixError::Empty);
        }
        for segment in raw.split(':') {
            if segment.is_empty() {
                return Err(HledgerAccountPrefixError::EmptySegment);
            }
            if let Some(character) = segment
                .chars()
                .find(|ch| !ch.is_ascii_alphanumeric() && !matches!(ch, '_' | '-' | ' '))
            {
                return Err(HledgerAccountPrefixError::DisallowedCharacter(character));
            }
            if segment.starts_with(' ') || segment.ends_with(' ') || segment.contains("  ") {
                return Err(HledgerAccountPrefixError::InvalidSpace);
            }
            if segment.len() > HLEDGER_ACCOUNT_SEGMENT_MAX_LENGTH {
                return Err(HledgerAccountPrefixError::SegmentTooLong);
            }
        }
        Ok(Self(raw.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for HledgerAccountPrefix {
    type Error = HledgerAccountPrefixError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl From<HledgerAccountPrefix> for String {
    fn from(value: HledgerAccountPrefix) -> Self {
        value.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HledgerAccountPrefixError {
    Empty,
    EmptySegment,
    InvalidSpace,
    SegmentTooLong,
    DisallowedCharacter(char),
}

impl fmt::Display for HledgerAccountPrefixError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(f, "Prefix must not be empty."),
            Self::EmptySegment => write!(
                f,
                "Prefix segments must not be empty (no leading, trailing, or double colons)."
            ),
            Self::InvalidSpace => write!(
                f,
                "Spaces are allowed only as single spaces inside a prefix segment."
            ),
            Self::SegmentTooLong => write!(
                f,
                "Each prefix segment must be at most {HLEDGER_ACCOUNT_SEGMENT_MAX_LENGTH} characters."
            ),
            Self::DisallowedCharacter(character) => write!(
                f,
                "Prefix may only contain letters, digits, single internal spaces, '_', '-', and ':' separators (found '{character}')."
            ),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EtherscanBaseUrl(Url);

impl EtherscanBaseUrl {
    pub(crate) fn parse(raw: &str) -> Result<Self, EtherscanBaseUrlError> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err(EtherscanBaseUrlError::Empty);
        }

        let mut url = Url::parse(trimmed)
            .map_err(|err| EtherscanBaseUrlError::InvalidUrl(err.to_string()))?;
        if url.cannot_be_a_base() {
            return Err(EtherscanBaseUrlError::NotABaseUrl);
        }

        match url.scheme() {
            "http" | "https" => {}
            other => {
                return Err(EtherscanBaseUrlError::UnsupportedScheme(other.to_string()));
            }
        }

        if url.query().is_some() {
            return Err(EtherscanBaseUrlError::QueryNotAllowed);
        }

        if url.fragment().is_some() {
            return Err(EtherscanBaseUrlError::FragmentNotAllowed);
        }

        if !url.path().ends_with('/') {
            let mut path = url.path().to_string();
            path.push('/');
            url.set_path(&path);
        }

        Ok(Self(url))
    }

    pub(crate) fn default_public() -> Self {
        match Self::parse(DEFAULT_ETHERSCAN_BASE_URL) {
            Ok(url) => url,
            Err(err) => {
                eprintln!("invalid DEFAULT_ETHERSCAN_BASE_URL: {err}");
                std::process::exit(1);
            }
        }
    }

    pub(crate) fn as_str(&self) -> &str {
        self.0.as_str()
    }

    /// Derive a web explorer root URL from the API base URL.
    ///
    /// Rules:
    /// 1. Take the scheme, host, and optional port.
    /// 2. If the host begins with `api.`, strip that prefix.
    /// 3. Drop the API path, query, and fragment.
    pub(crate) fn derive_web_explorer_root(&self) -> Result<Url, EtherscanBaseUrlError> {
        let host = self
            .0
            .host_str()
            .ok_or_else(|| EtherscanBaseUrlError::InvalidUrl("missing host".to_string()))?;
        let normalized_host = host.strip_prefix("api.").unwrap_or(host);

        let mut root = format!("{}://{}", self.0.scheme(), normalized_host);
        if let Some(port) = self.0.port() {
            root.push(':');
            root.push_str(&port.to_string());
        }
        root.push('/');

        Url::parse(&root).map_err(|err| EtherscanBaseUrlError::InvalidUrl(err.to_string()))
    }

    pub(crate) fn address_url(&self, address: &str) -> Result<String, EtherscanBaseUrlError> {
        self.derive_web_explorer_root()?
            .join(&format!("address/{address}"))
            .map(|url| url.to_string())
            .map_err(|err| EtherscanBaseUrlError::InvalidUrl(err.to_string()))
    }

    pub(crate) fn transaction_url(&self, tx_hash: &str) -> Result<String, EtherscanBaseUrlError> {
        self.derive_web_explorer_root()?
            .join(&format!("tx/{tx_hash}"))
            .map(|url| url.to_string())
            .map_err(|err| EtherscanBaseUrlError::InvalidUrl(err.to_string()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum EtherscanBaseUrlError {
    Empty,
    InvalidUrl(String),
    UnsupportedScheme(String),
    QueryNotAllowed,
    FragmentNotAllowed,
    NotABaseUrl,
}

impl fmt::Display for EtherscanBaseUrlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EtherscanBaseUrlError::Empty => write!(f, "Etherscan URL cannot be empty"),
            EtherscanBaseUrlError::InvalidUrl(message) => {
                write!(f, "Invalid Etherscan URL: {message}")
            }
            EtherscanBaseUrlError::UnsupportedScheme(scheme) => {
                write!(f, "Etherscan URL must use http or https, got '{scheme}'")
            }
            EtherscanBaseUrlError::QueryNotAllowed => {
                write!(f, "Etherscan URL cannot include query parameters")
            }
            EtherscanBaseUrlError::FragmentNotAllowed => {
                write!(f, "Etherscan URL cannot include URL fragments")
            }
            EtherscanBaseUrlError::NotABaseUrl => {
                write!(f, "Etherscan URL must be a base URL")
            }
        }
    }
}

impl std::error::Error for EtherscanBaseUrlError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EtherscanBaseUrlSource {
    DefaultPublic,
    UserOverride,
}

#[cfg(feature = "server")]
pub(crate) fn normalize_etherscan_base_url_override_for_storage(
    raw_override: Option<RawEtherscanBaseUrl>,
) -> Result<Option<EtherscanBaseUrl>, EtherscanBaseUrlError> {
    match raw_override {
        None => Ok(None),
        Some(raw) => {
            let trimmed = raw.as_str().trim();
            if trimmed.is_empty() {
                return Ok(None);
            }

            EtherscanBaseUrl::parse(trimmed).map(Some)
        }
    }
}

pub(crate) fn resolve_effective_etherscan_base_url(
    configured_override: Option<&RawEtherscanBaseUrl>,
) -> Result<(EtherscanBaseUrl, EtherscanBaseUrlSource), EtherscanBaseUrlError> {
    match configured_override {
        Some(override_url) => EtherscanBaseUrl::parse(override_url.as_str())
            .map(|parsed| (parsed, EtherscanBaseUrlSource::UserOverride)),
        None => Ok((
            EtherscanBaseUrl::default_public(),
            EtherscanBaseUrlSource::DefaultPublic,
        )),
    }
}

// ============ Etherscan API Key ============

/// Raw Etherscan API key from user settings (unvalidated beyond non-empty).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RawEtherscanApiKey(String);

impl RawEtherscanApiKey {
    pub(crate) fn new(value: String) -> Self {
        Self(value)
    }

    #[cfg(feature = "server")]
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

// ============ Simple API Keys ============

/// Provider identifiers supported by the simple API key store.
#[cfg(feature = "server")]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) enum ApiKeyProvider {
    Etherscan,
    CoinGecko,
}

#[cfg(feature = "server")]
impl ApiKeyProvider {
    pub(crate) fn as_storage_key(self) -> &'static str {
        match self {
            Self::Etherscan => "etherscan",
            Self::CoinGecko => "coingecko",
        }
    }

    pub(crate) fn from_storage_key(value: &str) -> Option<Self> {
        match value {
            "etherscan" => Some(Self::Etherscan),
            "coingecko" => Some(Self::CoinGecko),
            _ => None,
        }
    }
}

/// Stored simple provider API key.
#[derive(Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub(crate) struct SimpleApiKey(String);

impl std::fmt::Debug for SimpleApiKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SimpleApiKey(<redacted>)")
    }
}

impl SimpleApiKey {
    pub(crate) fn new(value: String) -> Option<Self> {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(Self(trimmed.to_string()))
        }
    }

    #[cfg(feature = "server")]
    pub(crate) fn from_non_empty_storage(value: String) -> Option<Self> {
        Self::new(value)
    }

    #[cfg(feature = "server")]
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for SimpleApiKey {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).ok_or_else(|| serde::de::Error::custom("API key must not be blank"))
    }
}

#[cfg(test)]
mod simple_api_key_debug_tests {
    use super::*;

    #[test]
    fn simple_api_key_debug_redacts_secret() {
        let debug = format!(
            "{:?}",
            SimpleApiKey::new("SECRET".to_string()).expect("test key")
        );

        assert!(!debug.contains("SECRET"));
        assert!(debug.contains("redacted"));
    }
}

#[cfg(feature = "server")]
impl From<SimpleApiKey> for RawEtherscanApiKey {
    fn from(value: SimpleApiKey) -> Self {
        RawEtherscanApiKey::new(value.0)
    }
}

#[cfg(all(test, feature = "server", not(bitgarth_db_unit_only)))]
mod simple_api_key_tests {
    use super::*;

    #[test]
    fn simple_api_key_rejects_blank_values() {
        assert!(SimpleApiKey::new("".to_string()).is_none());
        assert!(SimpleApiKey::new("   ".to_string()).is_none());
        assert_eq!(
            SimpleApiKey::new("  trimmed  ".to_string())
                .expect("trimmed key")
                .as_str(),
            "trimmed"
        );
    }
}

#[cfg(feature = "server")]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub(crate) struct SyncHistoryRetentionDays(u16);

#[cfg(feature = "server")]
impl SyncHistoryRetentionDays {
    #[cfg(all(test, feature = "db-tests"))]
    pub(crate) fn try_new(days: u16) -> Result<Self, SyncHistoryRetentionDaysError> {
        if days < 1 {
            return Err(SyncHistoryRetentionDaysError::TooShort);
        }
        if days > 365 {
            return Err(SyncHistoryRetentionDaysError::TooLong);
        }
        Ok(Self(days))
    }

    pub(crate) fn value(self) -> u16 {
        self.0
    }

    pub(crate) fn default_retention() -> Self {
        SyncHistoryRetentionDays(14)
    }
}

#[cfg(feature = "server")]
impl Default for SyncHistoryRetentionDays {
    fn default() -> Self {
        Self::default_retention()
    }
}

#[cfg(all(test, feature = "db-tests"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SyncHistoryRetentionDaysError {
    TooShort,
    TooLong,
}

#[cfg(all(test, feature = "db-tests"))]
impl fmt::Display for SyncHistoryRetentionDaysError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SyncHistoryRetentionDaysError::TooShort => {
                write!(f, "Retention must be at least 1 day")
            }
            SyncHistoryRetentionDaysError::TooLong => {
                write!(f, "Retention must be 365 days or less")
            }
        }
    }
}

#[cfg(all(test, feature = "db-tests"))]
impl std::error::Error for SyncHistoryRetentionDaysError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MempoolBaseUrl(Url);

impl MempoolBaseUrl {
    pub(crate) fn parse(raw: &str) -> Result<Self, MempoolBaseUrlError> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err(MempoolBaseUrlError::Empty);
        }

        let mut url =
            Url::parse(trimmed).map_err(|err| MempoolBaseUrlError::InvalidUrl(err.to_string()))?;
        if url.cannot_be_a_base() {
            return Err(MempoolBaseUrlError::NotABaseUrl);
        }

        match url.scheme() {
            "http" | "https" => {}
            other => {
                return Err(MempoolBaseUrlError::UnsupportedScheme(other.to_string()));
            }
        }

        if url.query().is_some() {
            return Err(MempoolBaseUrlError::QueryNotAllowed);
        }

        if url.fragment().is_some() {
            return Err(MempoolBaseUrlError::FragmentNotAllowed);
        }

        if !url.path().ends_with('/') {
            let mut path = url.path().to_string();
            path.push('/');
            url.set_path(&path);
        }

        Ok(Self(url))
    }

    pub(crate) fn default_public() -> Self {
        match Self::parse(DEFAULT_MEMPOOL_BASE_URL) {
            Ok(url) => url,
            Err(err) => {
                eprintln!("invalid DEFAULT_MEMPOOL_BASE_URL: {err}");
                std::process::exit(1);
            }
        }
    }

    pub(crate) fn as_str(&self) -> &str {
        self.0.as_str()
    }

    pub(crate) fn address_url(&self, address: &str) -> Result<String, MempoolBaseUrlError> {
        self.0
            .join(&format!("address/{address}"))
            .map(|url| url.to_string())
            .map_err(|err| MempoolBaseUrlError::InvalidUrl(err.to_string()))
    }

    pub(crate) fn transaction_url(&self, tx_hash: &str) -> Result<String, MempoolBaseUrlError> {
        self.0
            .join(&format!("tx/{tx_hash}"))
            .map(|url| url.to_string())
            .map_err(|err| MempoolBaseUrlError::InvalidUrl(err.to_string()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MempoolBaseUrlError {
    Empty,
    InvalidUrl(String),
    UnsupportedScheme(String),
    QueryNotAllowed,
    FragmentNotAllowed,
    NotABaseUrl,
}

impl fmt::Display for MempoolBaseUrlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MempoolBaseUrlError::Empty => write!(f, "Mempool URL cannot be empty"),
            MempoolBaseUrlError::InvalidUrl(message) => {
                write!(f, "Invalid mempool URL: {message}")
            }
            MempoolBaseUrlError::UnsupportedScheme(scheme) => {
                write!(f, "Mempool URL must use http or https, got '{scheme}'")
            }
            MempoolBaseUrlError::QueryNotAllowed => {
                write!(f, "Mempool URL cannot include query parameters")
            }
            MempoolBaseUrlError::FragmentNotAllowed => {
                write!(f, "Mempool URL cannot include URL fragments")
            }
            MempoolBaseUrlError::NotABaseUrl => {
                write!(f, "Mempool URL must be a base URL")
            }
        }
    }
}

impl std::error::Error for MempoolBaseUrlError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MempoolBaseUrlSource {
    DefaultPublic,
    UserOverride,
}

#[cfg(feature = "server")]
pub(crate) fn normalize_mempool_base_url_override_for_storage(
    raw_override: Option<RawMempoolBaseUrl>,
) -> Result<Option<MempoolBaseUrl>, MempoolBaseUrlError> {
    match raw_override {
        None => Ok(None),
        Some(raw) => {
            let trimmed = raw.as_str().trim();
            if trimmed.is_empty() {
                return Ok(None);
            }

            MempoolBaseUrl::parse(trimmed).map(Some)
        }
    }
}

pub(crate) fn resolve_effective_mempool_base_url(
    configured_override: Option<&RawMempoolBaseUrl>,
) -> Result<(MempoolBaseUrl, MempoolBaseUrlSource), MempoolBaseUrlError> {
    match configured_override {
        Some(override_url) => MempoolBaseUrl::parse(override_url.as_str())
            .map(|parsed| (parsed, MempoolBaseUrlSource::UserOverride)),
        None => Ok((
            MempoolBaseUrl::default_public(),
            MempoolBaseUrlSource::DefaultPublic,
        )),
    }
}

// ============ Session Duration ============

/// Default session duration in minutes (8 hours)
#[cfg(all(test, feature = "db-tests"))]
pub(crate) const DEFAULT_SESSION_DURATION_MINUTES: u32 = 480;

/// Session duration options.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum SessionDuration {
    OneHour,
    TwoHours,
    ThreeHours,
    FourHours,
    SixHours,
    #[default]
    EightHours,
    TwelveHours,
    TwentyFourHours,
    TwoDays,
    ThreeDays,
    SevenDays,
    FourteenDays,
    ThirtyDays,
}

impl SessionDuration {
    /// Get the duration in minutes.
    pub fn as_minutes(self) -> u32 {
        match self {
            SessionDuration::OneHour => 60,
            SessionDuration::TwoHours => 120,
            SessionDuration::ThreeHours => 180,
            SessionDuration::FourHours => 240,
            SessionDuration::SixHours => 360,
            SessionDuration::EightHours => 480,
            SessionDuration::TwelveHours => 720,
            SessionDuration::TwentyFourHours => 1440,
            SessionDuration::TwoDays => 2880,
            SessionDuration::ThreeDays => 4320,
            SessionDuration::SevenDays => 10080,
            SessionDuration::FourteenDays => 20160,
            SessionDuration::ThirtyDays => 43200,
        }
    }

    /// Database storage code.
    pub fn code(&self) -> String {
        self.as_minutes().to_string()
    }

    /// Parse from database code. Legacy `custom:N` values are mapped to the nearest preset.
    pub fn from_code(code: &str) -> Option<Self> {
        match code {
            "60" => Some(SessionDuration::OneHour),
            "120" => Some(SessionDuration::TwoHours),
            "180" => Some(SessionDuration::ThreeHours),
            "240" => Some(SessionDuration::FourHours),
            "360" => Some(SessionDuration::SixHours),
            "480" => Some(SessionDuration::EightHours),
            "720" => Some(SessionDuration::TwelveHours),
            "1440" => Some(SessionDuration::TwentyFourHours),
            "2880" => Some(SessionDuration::TwoDays),
            "4320" => Some(SessionDuration::ThreeDays),
            "10080" => Some(SessionDuration::SevenDays),
            "20160" => Some(SessionDuration::FourteenDays),
            "43200" => Some(SessionDuration::ThirtyDays),
            s if s.starts_with("custom:") => {
                let mins_str = s.strip_prefix("custom:")?;
                let mins: u32 = mins_str.parse().ok()?;
                Some(Self::nearest_preset(mins))
            }
            _ => None,
        }
    }

    /// Display label for UI.
    pub fn label(&self) -> &'static str {
        match self {
            SessionDuration::OneHour => "1 hour",
            SessionDuration::TwoHours => "2 hours",
            SessionDuration::ThreeHours => "3 hours",
            SessionDuration::FourHours => "4 hours",
            SessionDuration::SixHours => "6 hours",
            SessionDuration::EightHours => "8 hours (default)",
            SessionDuration::TwelveHours => "12 hours",
            SessionDuration::TwentyFourHours => "24 hours",
            SessionDuration::TwoDays => "2 days",
            SessionDuration::ThreeDays => "3 days",
            SessionDuration::SevenDays => "7 days",
            SessionDuration::FourteenDays => "14 days",
            SessionDuration::ThirtyDays => "30 days",
        }
    }

    /// All available options.
    pub fn all() -> &'static [SessionDuration] {
        &[
            SessionDuration::OneHour,
            SessionDuration::TwoHours,
            SessionDuration::ThreeHours,
            SessionDuration::FourHours,
            SessionDuration::SixHours,
            SessionDuration::EightHours,
            SessionDuration::TwelveHours,
            SessionDuration::TwentyFourHours,
            SessionDuration::TwoDays,
            SessionDuration::ThreeDays,
            SessionDuration::SevenDays,
            SessionDuration::FourteenDays,
            SessionDuration::ThirtyDays,
        ]
    }

    /// Find the preset closest in minutes to the given value.
    fn nearest_preset(minutes: u32) -> SessionDuration {
        let all = Self::all();
        let mut best = all[0];
        let mut best_diff = minutes.abs_diff(best.as_minutes());
        for &preset in &all[1..] {
            let diff = minutes.abs_diff(preset.as_minutes());
            if diff < best_diff {
                best = preset;
                best_diff = diff;
            }
        }
        best
    }
}

/// User settings stored in the per-user database
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct UserSettings {
    /// UI language (e.g., "en", "nl")
    pub language: Option<Locale>,
    /// Date/time format preference
    pub date_time_format: Option<DateTimeFormat>,
    /// Numeric display preference
    pub number_format: Option<NumberFormat>,
    /// Default currency
    pub currency: Option<CurrencyCode>,
    /// Preferred timezone
    pub timezone: Option<UserTimezone>,
    /// Session duration preference
    pub session_duration: Option<SessionDuration>,
    /// Optional user-configured mempool base URL override
    pub mempool_base_url: Option<RawMempoolBaseUrl>,
    /// Optional user-configured Etherscan base URL override
    pub etherscan_base_url: Option<RawEtherscanBaseUrl>,
    /// Optional custom hledger export account prefix replacing `assets:{owner}`
    #[serde(default)]
    pub hledger_account_prefix: Option<HledgerAccountPrefix>,
    /// Optional per-user Etherscan API key for Ethereum sync
    pub etherscan_api_key: Option<RawEtherscanApiKey>,
    /// Whether the user has configured an Etherscan API key (safe to expose to clients)
    pub has_etherscan_api_key: bool,
    /// Whether the user has configured a CoinGecko Pro API key (safe to expose to clients)
    #[serde(default)]
    pub has_coingecko_api_key: bool,
    /// Whether CoinGecko market-price fetching is enabled (sourced from the app db)
    #[serde(default)]
    pub price_fetching_enabled: bool,
}

// ============ Request/Response Types ============

/// Entry mode for unauthenticated routes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AuthEntryMode {
    Login,
    Register,
}

/// Banner kinds that can be returned from the auth entry decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AuthEntryBannerKind {
    DatabaseUnavailable,
}

/// Decision for the initial unauthenticated entry view.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AuthEntryDecision {
    pub mode: AuthEntryMode,
    pub banner: Option<AuthEntryBannerKind>,
}

impl Default for AuthEntryDecision {
    fn default() -> Self {
        Self {
            mode: AuthEntryMode::Register,
            banner: None,
        }
    }
}

/// Auth response with user details and settings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthResponse {
    pub user: User,
    pub settings: UserSettings,
}

// ============ Unit Tests ============

#[cfg(all(test, not(bitgarth_db_unit_only)))]
mod tests {
    use super::*;

    #[test]
    fn test_raw_username_no_validation() {
        // RawUsername can be created with any string
        let raw = RawUsername::new("".to_string());
        assert_eq!(raw.as_str(), "");

        let raw = RawUsername::new("user@email.com".to_string());
        assert_eq!(raw.as_str(), "user@email.com");
    }

    #[test]
    fn test_validated_username_valid() {
        let raw = RawUsername::new("valid_user-123".to_string());
        assert!(raw.validate().is_ok());

        let raw = RawUsername::new("a".to_string());
        assert!(raw.validate().is_ok());

        let raw = RawUsername::new("A".repeat(USERNAME_MAX_LENGTH));
        assert!(raw.validate().is_ok());
    }

    #[test]
    fn test_validated_username_email() {
        // Email addresses are valid usernames
        let raw = RawUsername::new("user@email.com".to_string());
        assert!(raw.validate().is_ok());
    }

    #[test]
    fn test_validated_username_empty() {
        let raw = RawUsername::new("".to_string());
        assert_eq!(raw.validate(), Err(UsernameError::Empty));
    }

    #[test]
    fn test_validated_username_too_long() {
        let raw = RawUsername::new("a".repeat(USERNAME_MAX_LENGTH + 1));
        assert_eq!(raw.validate(), Err(UsernameError::TooLong));
    }

    #[test]
    fn test_validated_username_invalid_characters() {
        let raw = RawUsername::new("user with spaces".to_string());
        assert_eq!(raw.validate(), Err(UsernameError::InvalidCharacters));
    }

    // — Server-only tests —
    #[cfg(feature = "server")]
    #[test]
    fn test_raw_password_no_validation() {
        // RawPlaintextPassword can be created with any string
        let raw = RawPlaintextPassword::new("weak".to_string());
        assert_eq!(raw.as_str(), "weak");
    }

    #[cfg(feature = "server")]
    #[test]
    fn test_validated_password_valid() {
        let raw = RawPlaintextPassword::new("SecurePass123".to_string());
        assert!(raw.validate_all().is_ok());
    }

    #[cfg(feature = "server")]
    #[test]
    fn test_validated_password_too_short() {
        let raw = RawPlaintextPassword::new("Short1".to_string());
        let errs = raw.validate_all().unwrap_err();
        assert!(errs.contains(&PasswordValidationError::TooShort));
    }

    #[cfg(feature = "server")]
    #[test]
    fn test_validated_password_missing_uppercase() {
        let raw = RawPlaintextPassword::new("lowercase123".to_string());
        let errs = raw.validate_all().unwrap_err();
        assert!(errs.contains(&PasswordValidationError::MissingUppercase));
    }

    #[cfg(feature = "server")]
    #[test]
    fn test_validated_password_missing_lowercase() {
        let raw = RawPlaintextPassword::new("UPPERCASE123".to_string());
        let errs = raw.validate_all().unwrap_err();
        assert!(errs.contains(&PasswordValidationError::MissingLowercase));
    }

    #[cfg(feature = "server")]
    #[test]
    fn test_validated_password_missing_number() {
        let raw = RawPlaintextPassword::new("NoNumberHere".to_string());
        let errs = raw.validate_all().unwrap_err();
        assert!(errs.contains(&PasswordValidationError::MissingNumber));
    }

    #[test]
    fn test_username_serde_transparent() {
        let username = RawUsername::new("testuser".to_string()).validate().unwrap();
        let json = serde_json::to_string(&username).unwrap();
        assert_eq!(json, "\"testuser\"");

        let raw: RawUsername = serde_json::from_str("\"testuser\"").unwrap();
        assert_eq!(raw.as_str(), "testuser");
    }

    #[cfg(feature = "server")]
    #[test]
    fn test_password_serde_transparent() {
        let password = RawPlaintextPassword::new("secret123".to_string());
        let json = serde_json::to_string(&password).unwrap();
        assert_eq!(json, "\"secret123\"");

        let parsed: RawPlaintextPassword = serde_json::from_str("\"secret123\"").unwrap();
        assert_eq!(parsed.as_str(), "secret123");
    }

    #[cfg(feature = "server")]
    #[test]
    fn test_session_token_serde_transparent() {
        let token = SessionToken::from_raw("abc123token".to_string());
        let json = serde_json::to_string(&token).unwrap();
        assert_eq!(json, "\"abc123token\"");

        let parsed: SessionToken = serde_json::from_str("\"abc123token\"").unwrap();
        assert_eq!(parsed.as_str(), "abc123token");
    }

    #[cfg(feature = "server")]
    #[test]
    fn test_datetime_parsing_rfc3339() {
        let valid = "2024-01-15T10:30:00Z";
        assert!(parse_datetime(valid).is_ok());
    }

    #[cfg(feature = "server")]
    #[test]
    fn test_datetime_parsing_sqlite_format() {
        // SQLite's datetime('now') returns this format
        let sqlite_format = "2024-01-15 10:30:00";
        assert!(parse_datetime(sqlite_format).is_ok());
    }

    #[cfg(feature = "server")]
    #[test]
    fn test_datetime_parsing_invalid() {
        let invalid = "not a date";
        assert!(parse_datetime(invalid).is_err());
    }

    #[test]
    fn test_field_errors() {
        let mut errors = FieldErrors::new();
        assert!(errors.is_empty());

        errors.add("username", "Username is required".to_string());
        errors.add("password", "Password too short".to_string());
        errors.add("password", "Password needs uppercase".to_string());

        assert!(!errors.is_empty());
        assert_eq!(
            errors.first("username"),
            Some(&"Username is required".to_string())
        );
        assert_eq!(errors.get("password").map(|v| v.len()), Some(2));
    }

    #[cfg(feature = "server")]
    #[test]
    fn test_normalize_mempool_base_url_override_for_storage_accepts_clear() {
        let normalized = normalize_mempool_base_url_override_for_storage(Some(
            RawMempoolBaseUrl::new("   ".to_string()),
        ))
        .expect("blank override should clear configuration");
        assert!(normalized.is_none());
    }

    #[test]
    fn test_resolve_effective_mempool_base_url_uses_override_without_fallback() {
        let configured = RawMempoolBaseUrl::new("https://example.com/custom".to_string());
        let (resolved, source) = resolve_effective_mempool_base_url(Some(&configured))
            .expect("configured override should parse");

        assert_eq!(source, MempoolBaseUrlSource::UserOverride);
        assert_eq!(resolved.as_str(), "https://example.com/custom/");
    }

    #[test]
    fn test_resolve_effective_mempool_base_url_rejects_invalid_override() {
        let configured = RawMempoolBaseUrl::new("not a url".to_string());
        let err = resolve_effective_mempool_base_url(Some(&configured))
            .expect_err("invalid configured URL must fail");

        assert!(matches!(err, MempoolBaseUrlError::InvalidUrl(_)));
    }

    // ============ Etherscan Base URL Tests ============

    #[test]
    fn test_etherscan_base_url_valid_https() {
        let url = EtherscanBaseUrl::parse("https://api.etherscan.io/v2/api")
            .expect("valid https URL should parse");
        assert_eq!(url.as_str(), "https://api.etherscan.io/v2/api/");
    }

    #[test]
    fn test_etherscan_base_url_valid_http() {
        let url =
            EtherscanBaseUrl::parse("http://localhost:3000").expect("valid http URL should parse");
        assert_eq!(url.as_str(), "http://localhost:3000/");
    }

    #[test]
    fn test_etherscan_base_url_trailing_slash_normalized() {
        let url =
            EtherscanBaseUrl::parse("https://example.com/api").expect("should parse without slash");
        assert!(url.as_str().ends_with('/'));

        let url2 =
            EtherscanBaseUrl::parse("https://example.com/api/").expect("should parse with slash");
        assert_eq!(url.as_str(), url2.as_str());
    }

    #[test]
    fn test_etherscan_base_url_empty() {
        let err = EtherscanBaseUrl::parse("").expect_err("empty should fail");
        assert_eq!(err, EtherscanBaseUrlError::Empty);
    }

    #[test]
    fn test_etherscan_base_url_whitespace_only() {
        let err = EtherscanBaseUrl::parse("   ").expect_err("whitespace should fail");
        assert_eq!(err, EtherscanBaseUrlError::Empty);
    }

    #[test]
    fn test_etherscan_base_url_unsupported_scheme() {
        let err = EtherscanBaseUrl::parse("ftp://example.com").expect_err("ftp should fail");
        assert!(matches!(err, EtherscanBaseUrlError::UnsupportedScheme(_)));
    }

    #[test]
    fn test_etherscan_base_url_rejects_query_params() {
        let err =
            EtherscanBaseUrl::parse("https://example.com?foo=bar").expect_err("query should fail");
        assert_eq!(err, EtherscanBaseUrlError::QueryNotAllowed);
    }

    #[test]
    fn test_etherscan_base_url_rejects_fragment() {
        let err = EtherscanBaseUrl::parse("https://example.com#section")
            .expect_err("fragment should fail");
        assert_eq!(err, EtherscanBaseUrlError::FragmentNotAllowed);
    }

    #[test]
    fn test_etherscan_base_url_invalid_url() {
        let err = EtherscanBaseUrl::parse("not a url").expect_err("invalid should fail");
        assert!(matches!(err, EtherscanBaseUrlError::InvalidUrl(_)));
    }

    #[cfg(feature = "server")]
    #[test]
    fn test_normalize_etherscan_base_url_override_for_storage_accepts_clear() {
        let normalized = normalize_etherscan_base_url_override_for_storage(Some(
            RawEtherscanBaseUrl::new("   ".to_string()),
        ))
        .expect("blank override should clear configuration");
        assert!(normalized.is_none());
    }

    #[cfg(feature = "server")]
    #[test]
    fn test_normalize_etherscan_base_url_override_for_storage_none() {
        let normalized = normalize_etherscan_base_url_override_for_storage(None)
            .expect("None should clear configuration");
        assert!(normalized.is_none());
    }

    #[cfg(feature = "server")]
    #[test]
    fn test_normalize_etherscan_base_url_override_for_storage_valid() {
        let normalized = normalize_etherscan_base_url_override_for_storage(Some(
            RawEtherscanBaseUrl::new("http://localhost:3000".to_string()),
        ))
        .expect("valid URL should normalize");
        assert!(normalized.is_some());
        assert_eq!(normalized.unwrap().as_str(), "http://localhost:3000/");
    }

    #[test]
    fn test_resolve_effective_etherscan_base_url_default() {
        let (resolved, source) =
            resolve_effective_etherscan_base_url(None).expect("default should resolve");
        assert_eq!(source, EtherscanBaseUrlSource::DefaultPublic);
        assert_eq!(resolved.as_str(), "https://api.etherscan.io/v2/api/");
    }

    #[test]
    fn test_resolve_effective_etherscan_base_url_override() {
        let configured = RawEtherscanBaseUrl::new("http://localhost:9000".to_string());
        let (resolved, source) = resolve_effective_etherscan_base_url(Some(&configured))
            .expect("configured override should parse");
        assert_eq!(source, EtherscanBaseUrlSource::UserOverride);
        assert_eq!(resolved.as_str(), "http://localhost:9000/");
    }

    #[test]
    fn test_resolve_effective_etherscan_base_url_rejects_invalid_override() {
        let configured = RawEtherscanBaseUrl::new("not a url".to_string());
        let err = resolve_effective_etherscan_base_url(Some(&configured))
            .expect_err("invalid configured URL must fail");
        assert!(matches!(err, EtherscanBaseUrlError::InvalidUrl(_)));
    }

    #[test]
    fn hledger_account_prefix_accepts_valid_prefixes() {
        for raw in [
            "assets",
            "assets:Liquid:Crypto",
            "assets:My Wallet",
            "assets:Liquid Crypto:Cold Storage",
            "crypto:holdings",
            "a_b-c:d1",
        ] {
            let prefix = HledgerAccountPrefix::parse(raw).expect("prefix should parse");
            assert_eq!(prefix.as_str(), raw);
        }
        let max_segment = "a".repeat(HLEDGER_ACCOUNT_SEGMENT_MAX_LENGTH);
        assert!(HledgerAccountPrefix::parse(&max_segment).is_ok());
    }

    #[test]
    fn hledger_account_prefix_rejects_invalid_prefixes() {
        assert_eq!(
            HledgerAccountPrefix::parse(""),
            Err(HledgerAccountPrefixError::Empty)
        );
        for raw in [":assets", "assets:", "assets::x"] {
            assert_eq!(
                HledgerAccountPrefix::parse(raw),
                Err(HledgerAccountPrefixError::EmptySegment)
            );
        }
        assert_eq!(
            HledgerAccountPrefix::parse("assets: MyWallet"),
            Err(HledgerAccountPrefixError::InvalidSpace)
        );
        assert_eq!(
            HledgerAccountPrefix::parse("assets:MyWallet "),
            Err(HledgerAccountPrefixError::InvalidSpace)
        );
        assert_eq!(
            HledgerAccountPrefix::parse("assets:My  Wallet"),
            Err(HledgerAccountPrefixError::InvalidSpace)
        );
        assert_eq!(
            HledgerAccountPrefix::parse("assets:猫"),
            Err(HledgerAccountPrefixError::DisallowedCharacter('猫'))
        );
        let too_long = "a".repeat(HLEDGER_ACCOUNT_SEGMENT_MAX_LENGTH + 1);
        assert_eq!(
            HledgerAccountPrefix::parse(&too_long),
            Err(HledgerAccountPrefixError::SegmentTooLong)
        );
    }

    #[test]
    fn hledger_account_prefix_serde_round_trips_as_string() {
        let prefix = HledgerAccountPrefix::parse("assets:Liquid:Crypto").expect("should parse");
        let json = serde_json::to_string(&prefix).expect("serialize should work");
        assert_eq!(json, "\"assets:Liquid:Crypto\"");
        let parsed: HledgerAccountPrefix =
            serde_json::from_str(&json).expect("deserialize should work");
        assert_eq!(parsed, prefix);
        assert!(serde_json::from_str::<HledgerAccountPrefix>("\"bad  prefix\"").is_err());
    }

    // ============ SessionDuration Tests ============

    #[test]
    fn test_session_duration_code_roundtrip() {
        for &duration in SessionDuration::all() {
            let code = duration.code();
            let parsed = SessionDuration::from_code(&code);
            assert_eq!(parsed, Some(duration), "roundtrip failed for {code}");
        }
    }

    #[test]
    fn test_session_duration_legacy_custom_maps_to_nearest() {
        // custom:100 is closest to TwoHours (120min, diff=20) vs OneHour (60min, diff=40)
        assert_eq!(
            SessionDuration::from_code("custom:100"),
            Some(SessionDuration::TwoHours)
        );
        // custom:500 is closest to EightHours (480min)
        assert_eq!(
            SessionDuration::from_code("custom:500"),
            Some(SessionDuration::EightHours)
        );
        // custom:10000 is closest to SevenDays (10080min)
        assert_eq!(
            SessionDuration::from_code("custom:10000"),
            Some(SessionDuration::SevenDays)
        );
    }

    #[test]
    fn test_session_duration_unknown_code_returns_none() {
        assert_eq!(SessionDuration::from_code("999"), None);
        assert_eq!(SessionDuration::from_code(""), None);
        assert_eq!(SessionDuration::from_code("custom:"), None);
        assert_eq!(SessionDuration::from_code("custom:abc"), None);
    }
}
