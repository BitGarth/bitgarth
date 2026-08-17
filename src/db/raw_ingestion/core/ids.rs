use crate::db::error::DbError;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;
use ulid::Ulid;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub(crate) struct SyncRunId(Ulid);

impl SyncRunId {
    pub(crate) fn new() -> Self {
        Self(Ulid::new())
    }
}

impl fmt::Debug for SyncRunId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("SyncRunId")
            .field(&self.0.to_string())
            .finish()
    }
}

impl fmt::Display for SyncRunId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for SyncRunId {
    type Err = ulid::DecodeError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Ulid::from_string(value).map(Self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct SourceConnectionId(String);

impl SourceConnectionId {
    pub(crate) fn new() -> Self {
        Self(Ulid::new().to_string())
    }

    pub(super) fn parse(raw: &str) -> Result<Self, DbError> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err(DbError::new("source connection id cannot be empty"));
        }
        Ok(Self(trimmed.to_string()))
    }
}

impl fmt::Display for SourceConnectionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for SourceConnectionId {
    type Err = DbError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct RequestAttemptId(Ulid);

impl RequestAttemptId {
    pub(crate) fn new() -> Self {
        Self(Ulid::new())
    }
}

impl fmt::Debug for RequestAttemptId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("RequestAttemptId")
            .field(&self.0.to_string())
            .finish()
    }
}

impl fmt::Display for RequestAttemptId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct RawObservationSetId(Ulid);

impl RawObservationSetId {
    pub(crate) fn new() -> Self {
        Self(Ulid::new())
    }
}

impl fmt::Debug for RawObservationSetId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("RawObservationSetId")
            .field(&self.0.to_string())
            .finish()
    }
}

impl fmt::Display for RawObservationSetId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for RawObservationSetId {
    type Err = ulid::DecodeError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Ulid::from_string(value).map(Self)
    }
}

impl FromStr for RequestAttemptId {
    type Err = ulid::DecodeError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Ulid::from_string(value).map(Self)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct RawMempoolTransactionVersionId(Ulid);

impl RawMempoolTransactionVersionId {
    pub(crate) fn new() -> Self {
        Self(Ulid::new())
    }
}

impl fmt::Debug for RawMempoolTransactionVersionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("RawMempoolTransactionVersionId")
            .field(&self.0.to_string())
            .finish()
    }
}

impl fmt::Display for RawMempoolTransactionVersionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for RawMempoolTransactionVersionId {
    type Err = ulid::DecodeError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Ulid::from_string(value).map(Self)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct RawMempoolTransactionObservationId(Ulid);

impl RawMempoolTransactionObservationId {
    pub(crate) fn new() -> Self {
        Self(Ulid::new())
    }
}

impl fmt::Debug for RawMempoolTransactionObservationId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("RawMempoolTransactionObservationId")
            .field(&self.0.to_string())
            .finish()
    }
}

impl fmt::Display for RawMempoolTransactionObservationId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct RawEtherscanNormalTransactionVersionId(Ulid);

impl RawEtherscanNormalTransactionVersionId {
    pub(crate) fn new() -> Self {
        Self(Ulid::new())
    }
}

impl fmt::Debug for RawEtherscanNormalTransactionVersionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("RawEtherscanNormalTransactionVersionId")
            .field(&self.0.to_string())
            .finish()
    }
}

impl fmt::Display for RawEtherscanNormalTransactionVersionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for RawEtherscanNormalTransactionVersionId {
    type Err = ulid::DecodeError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Ulid::from_string(value).map(Self)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct RawEtherscanInternalTransactionVersionId(Ulid);

impl RawEtherscanInternalTransactionVersionId {
    pub(crate) fn new() -> Self {
        Self(Ulid::new())
    }
}

impl fmt::Debug for RawEtherscanInternalTransactionVersionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("RawEtherscanInternalTransactionVersionId")
            .field(&self.0.to_string())
            .finish()
    }
}

impl fmt::Display for RawEtherscanInternalTransactionVersionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for RawEtherscanInternalTransactionVersionId {
    type Err = ulid::DecodeError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Ulid::from_string(value).map(Self)
    }
}

#[cfg(all(test, feature = "db-tests"))]
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct RawEtherscanNormalTransactionObservationId(Ulid);

#[cfg(all(test, feature = "db-tests"))]
impl RawEtherscanNormalTransactionObservationId {
    pub(crate) fn new() -> Self {
        Self(Ulid::new())
    }
}

#[cfg(all(test, feature = "db-tests"))]
impl fmt::Debug for RawEtherscanNormalTransactionObservationId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("RawEtherscanNormalTransactionObservationId")
            .field(&self.0.to_string())
            .finish()
    }
}

#[cfg(all(test, feature = "db-tests"))]
impl fmt::Display for RawEtherscanNormalTransactionObservationId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[cfg(all(test, feature = "db-tests"))]
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct RawEtherscanInternalTransactionObservationId(Ulid);

#[cfg(all(test, feature = "db-tests"))]
impl RawEtherscanInternalTransactionObservationId {
    pub(crate) fn new() -> Self {
        Self(Ulid::new())
    }
}

#[cfg(all(test, feature = "db-tests"))]
impl fmt::Debug for RawEtherscanInternalTransactionObservationId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("RawEtherscanInternalTransactionObservationId")
            .field(&self.0.to_string())
            .finish()
    }
}

#[cfg(all(test, feature = "db-tests"))]
impl fmt::Display for RawEtherscanInternalTransactionObservationId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}
