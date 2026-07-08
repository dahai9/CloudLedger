#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct CurrencyCode(String);

impl CurrencyCode {
    pub fn new(value: impl Into<String>) -> Self {
        Self::try_new(value).expect("currency code must be a three-letter ISO style code")
    }

    pub fn try_new(value: impl Into<String>) -> Result<Self, CurrencyCodeError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(CurrencyCodeError::Empty);
        }

        let normalized = value.trim().to_ascii_uppercase();
        if normalized.len() != 3 || !normalized.chars().all(|c| c.is_ascii_uppercase()) {
            return Err(CurrencyCodeError::InvalidFormat(value));
        }

        Ok(Self(normalized))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for CurrencyCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CurrencyCodeError {
    Empty,
    InvalidFormat(String),
}

impl std::fmt::Display for CurrencyCodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CurrencyCodeError::Empty => f.write_str("currency code cannot be empty"),
            CurrencyCodeError::InvalidFormat(value) => {
                write!(f, "invalid currency code `{value}`")
            }
        }
    }
}

impl std::error::Error for CurrencyCodeError {}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Money {
    pub currency: CurrencyCode,
    pub minor_units: i64,
}

impl Money {
    pub fn new(currency: CurrencyCode, minor_units: i64) -> Self {
        Self {
            currency,
            minor_units,
        }
    }

    pub fn zero(currency: CurrencyCode) -> Self {
        Self::new(currency, 0)
    }

    pub fn is_positive(&self) -> bool {
        self.minor_units > 0
    }

    pub fn same_currency(&self, other: &Self) -> bool {
        self.currency == other.currency
    }

    pub fn add_same_currency(&self, other: &Self) -> Option<Self> {
        if !self.same_currency(other) {
            return None;
        }

        self.minor_units
            .checked_add(other.minor_units)
            .map(|minor_units| Self::new(self.currency.clone(), minor_units))
    }
}
