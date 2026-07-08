use crate::amount::{CurrencyCode, Money};
use crate::ids::{CompanyId, LedgerId, UserId};

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum LedgerKind {
    PersonalPrivate { owner_id: UserId },
    CompanyPublic { company_id: CompanyId },
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ApprovalPolicy {
    pub company_entries_require_approval: bool,
    pub high_value_threshold: Money,
    pub min_approvals: u8,
}

impl ApprovalPolicy {
    pub fn personal(base_currency: CurrencyCode) -> Self {
        Self {
            company_entries_require_approval: false,
            high_value_threshold: Money::new(base_currency, i64::MAX),
            min_approvals: 0,
        }
    }

    pub fn company(high_value_threshold: Money) -> Self {
        Self {
            company_entries_require_approval: true,
            high_value_threshold,
            min_approvals: 1,
        }
    }

    pub fn with_min_approvals(mut self, min_approvals: u8) -> Self {
        self.min_approvals = min_approvals.max(1);
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Ledger {
    pub id: LedgerId,
    pub name: String,
    pub base_currency: CurrencyCode,
    pub kind: LedgerKind,
    pub approval_policy: ApprovalPolicy,
}

impl Ledger {
    pub fn personal(
        id: LedgerId,
        owner_id: UserId,
        name: impl Into<String>,
        base_currency: CurrencyCode,
    ) -> Self {
        Self {
            id,
            name: name.into(),
            approval_policy: ApprovalPolicy::personal(base_currency.clone()),
            base_currency,
            kind: LedgerKind::PersonalPrivate { owner_id },
        }
    }

    pub fn company(
        id: LedgerId,
        company_id: CompanyId,
        name: impl Into<String>,
        base_currency: CurrencyCode,
        high_value_threshold: Money,
    ) -> Self {
        assert!(
            base_currency == high_value_threshold.currency,
            "company approval threshold must use the ledger base currency"
        );

        Self {
            id,
            name: name.into(),
            base_currency,
            kind: LedgerKind::CompanyPublic { company_id },
            approval_policy: ApprovalPolicy::company(high_value_threshold),
        }
    }
}
