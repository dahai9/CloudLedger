use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Money {
    pub amount_minor: i64,
    pub currency: String,
}

impl Money {
    pub fn new(amount_minor: i64, currency: impl Into<String>) -> Result<Self, MoneyError> {
        let currency = currency.into().trim().to_uppercase();
        if currency.len() != 3 || !currency.chars().all(|ch| ch.is_ascii_uppercase()) {
            return Err(MoneyError::InvalidCurrency);
        }

        Ok(Self {
            amount_minor,
            currency,
        })
    }

    pub fn zero(currency: impl Into<String>) -> Result<Self, MoneyError> {
        Self::new(0, currency)
    }

    pub fn checked_add(&self, other: &Money) -> Result<Money, MoneyError> {
        if self.currency != other.currency {
            return Err(MoneyError::CurrencyMismatch);
        }

        Ok(Money {
            amount_minor: self
                .amount_minor
                .checked_add(other.amount_minor)
                .ok_or(MoneyError::Overflow)?,
            currency: self.currency.clone(),
        })
    }

    pub fn absolute(&self) -> Money {
        Money {
            amount_minor: self.amount_minor.abs(),
            currency: self.currency.clone(),
        }
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum MoneyError {
    #[error("currency must be a three-letter ISO code")]
    InvalidCurrency,
    #[error("cannot combine different currencies")]
    CurrencyMismatch,
    #[error("money amount overflowed")]
    Overflow,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_invalid_currency() {
        assert_eq!(
            Money::new(100, "cn").unwrap_err(),
            MoneyError::InvalidCurrency
        );
        assert_eq!(
            Money::new(100, "CNY1").unwrap_err(),
            MoneyError::InvalidCurrency
        );
    }

    #[test]
    fn adds_only_same_currency() {
        let base = Money::new(1200, "CNY").unwrap();
        let next = Money::new(300, "CNY").unwrap();
        assert_eq!(base.checked_add(&next).unwrap().amount_minor, 1500);

        let usd = Money::new(300, "USD").unwrap();
        assert_eq!(
            base.checked_add(&usd).unwrap_err(),
            MoneyError::CurrencyMismatch
        );
    }
}
