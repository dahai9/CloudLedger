use crate::amount::{CurrencyCode, Money};
use crate::ids::{AccountId, JournalEntryId, LedgerId, UserId};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum AccountKind {
    Asset,
    Liability,
    Equity,
    Revenue,
    Expense,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Account {
    pub id: AccountId,
    pub ledger_id: LedgerId,
    pub code: String,
    pub name: String,
    pub kind: AccountKind,
}

impl Account {
    pub fn new(
        id: AccountId,
        ledger_id: LedgerId,
        code: impl Into<String>,
        name: impl Into<String>,
        kind: AccountKind,
    ) -> Self {
        Self {
            id,
            ledger_id,
            code: code.into(),
            name: name.into(),
            kind,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum PostingSide {
    Debit,
    Credit,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct JournalLine {
    pub account_id: AccountId,
    pub side: PostingSide,
    pub amount: Money,
    pub memo: Option<String>,
}

impl JournalLine {
    pub fn new(
        account_id: AccountId,
        side: PostingSide,
        amount: Money,
        memo: Option<String>,
    ) -> Self {
        Self {
            account_id,
            side,
            amount,
            memo,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum JournalStatus {
    Draft,
    PendingApproval,
    Posted,
    Rejected,
    Voided,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct JournalEntry {
    pub id: JournalEntryId,
    pub ledger_id: LedgerId,
    pub created_by: UserId,
    pub description: String,
    pub lines: Vec<JournalLine>,
    pub status: JournalStatus,
}

impl JournalEntry {
    pub fn new(
        id: JournalEntryId,
        ledger_id: LedgerId,
        created_by: UserId,
        description: impl Into<String>,
        lines: Vec<JournalLine>,
    ) -> Self {
        Self {
            id,
            ledger_id,
            created_by,
            description: description.into(),
            lines,
            status: JournalStatus::Draft,
        }
    }

    pub fn validate(&self) -> Result<(), JournalValidationError> {
        if self.lines.len() < 2 {
            return Err(JournalValidationError::TooFewLines);
        }

        let currency = self
            .currency()
            .ok_or(JournalValidationError::TooFewLines)?
            .clone();
        let mut debit_total: i128 = 0;
        let mut credit_total: i128 = 0;

        for line in &self.lines {
            if !line.amount.is_positive() {
                return Err(JournalValidationError::NonPositiveAmount);
            }

            if line.amount.currency != currency {
                return Err(JournalValidationError::MixedCurrencies);
            }

            match line.side {
                PostingSide::Debit => debit_total += i128::from(line.amount.minor_units),
                PostingSide::Credit => credit_total += i128::from(line.amount.minor_units),
            }
        }

        if debit_total != credit_total {
            return Err(JournalValidationError::Unbalanced {
                debit_total,
                credit_total,
            });
        }

        Ok(())
    }

    pub fn currency(&self) -> Option<&CurrencyCode> {
        self.lines.first().map(|line| &line.amount.currency)
    }

    pub fn largest_line_amount(&self) -> Option<Money> {
        self.lines
            .iter()
            .map(|line| line.amount.clone())
            .max_by_key(|amount| amount.minor_units)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JournalValidationError {
    TooFewLines,
    NonPositiveAmount,
    MixedCurrencies,
    Unbalanced {
        debit_total: i128,
        credit_total: i128,
    },
}

impl std::fmt::Display for JournalValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            JournalValidationError::TooFewLines => {
                f.write_str("a journal entry must contain at least two lines")
            }
            JournalValidationError::NonPositiveAmount => {
                f.write_str("journal line amounts must be positive")
            }
            JournalValidationError::MixedCurrencies => {
                f.write_str("all journal lines must use the same currency")
            }
            JournalValidationError::Unbalanced {
                debit_total,
                credit_total,
            } => write!(
                f,
                "journal entry is not balanced: debits={debit_total}, credits={credit_total}"
            ),
        }
    }
}

impl std::error::Error for JournalValidationError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_double_entry_balance() {
        let entry = JournalEntry::new(
            JournalEntryId::from("entry"),
            LedgerId::from("ledger"),
            UserId::from("owner"),
            "Lunch",
            vec![
                JournalLine::new(
                    AccountId::from("expense"),
                    PostingSide::Debit,
                    Money::new(CurrencyCode::new("CNY"), 3_500),
                    None,
                ),
                JournalLine::new(
                    AccountId::from("cash"),
                    PostingSide::Credit,
                    Money::new(CurrencyCode::new("CNY"), 3_500),
                    None,
                ),
            ],
        );

        assert_eq!(entry.validate(), Ok(()));
    }

    #[test]
    fn rejects_unbalanced_entries() {
        let entry = JournalEntry::new(
            JournalEntryId::from("entry"),
            LedgerId::from("ledger"),
            UserId::from("owner"),
            "Lunch",
            vec![
                JournalLine::new(
                    AccountId::from("expense"),
                    PostingSide::Debit,
                    Money::new(CurrencyCode::new("CNY"), 3_500),
                    None,
                ),
                JournalLine::new(
                    AccountId::from("cash"),
                    PostingSide::Credit,
                    Money::new(CurrencyCode::new("CNY"), 3_000),
                    None,
                ),
            ],
        );

        assert!(matches!(
            entry.validate(),
            Err(JournalValidationError::Unbalanced { .. })
        ));
    }
}
