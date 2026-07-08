use cloudledger_core::{ApprovalDecisionError, JournalValidationError};
use cloudledger_db::RepositoryError;

#[derive(Debug)]
pub enum ServiceError {
    Repository(RepositoryError),
    LedgerNotFound,
    JournalEntryNotFound,
    ApprovalRequestNotFound,
    ActorMismatch,
    PermissionDenied,
    InvalidJournal(JournalValidationError),
    ApprovalDecision(ApprovalDecisionError),
    SelfApprovalDenied,
    RoleNotAccepted,
}

impl std::fmt::Display for ServiceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ServiceError::Repository(error) => write!(f, "repository error: {error}"),
            ServiceError::LedgerNotFound => f.write_str("ledger not found"),
            ServiceError::JournalEntryNotFound => f.write_str("journal entry not found"),
            ServiceError::ApprovalRequestNotFound => f.write_str("approval request not found"),
            ServiceError::ActorMismatch => {
                f.write_str("journal entry creator must match the submitting actor")
            }
            ServiceError::PermissionDenied => f.write_str("permission denied"),
            ServiceError::InvalidJournal(error) => write!(f, "invalid journal entry: {error}"),
            ServiceError::ApprovalDecision(error) => write!(f, "approval decision error: {error}"),
            ServiceError::SelfApprovalDenied => f.write_str("submitter cannot approve own entry"),
            ServiceError::RoleNotAccepted => {
                f.write_str("actor role does not satisfy the approval requirement")
            }
        }
    }
}

impl std::error::Error for ServiceError {}

impl From<RepositoryError> for ServiceError {
    fn from(value: RepositoryError) -> Self {
        ServiceError::Repository(value)
    }
}

impl From<JournalValidationError> for ServiceError {
    fn from(value: JournalValidationError) -> Self {
        ServiceError::InvalidJournal(value)
    }
}

impl From<ApprovalDecisionError> for ServiceError {
    fn from(value: ApprovalDecisionError) -> Self {
        ServiceError::ApprovalDecision(value)
    }
}
