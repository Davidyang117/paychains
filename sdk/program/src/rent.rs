#![allow(clippy::integer_arithmetic)]
//! configuration for network rent
use crate::clock::DEFAULT_SLOTS_PER_EPOCH;

#[repr(C)]
#[derive(Serialize, Deserialize, PartialEq, Clone, Copy, Debug, AbiExample)]
pub struct Rent {
    /// Rental rate
    pub lamports_per_byte_year: u64,

    /// exemption threshold, in years
    pub exemption_threshold: f64,

    // What portion of collected rent are to be destroyed, percentage-wise
    pub burn_percent: u8,
}

/// default rental rate in lamports/byte-year, based on:
///  10^9 lamports per PAY
///  $1 per PAY
///  $0.01 per megabyte day
///  $3.65 per megabyte year
pub const DEFAULT_LAMPORTS_PER_BYTE_YEAR: u64 = 1_000_000_000 / 100 * 365 / (1024 * 1024);

/// default amount of time (in years) the balance has to include rent for
pub const DEFAULT_EXEMPTION_THRESHOLD: f64 = 2.0;

/// default percentage of rent to burn (Valid values are 0 to 100)
pub const DEFAULT_BURN_PERCENT: u8 = 50;

/// account storage overhead for calculation of base rent
pub const ACCOUNT_STORAGE_OVERHEAD: u64 = 128;

impl Default for Rent {
    fn default() -> Self {
        Self {
            lamports_per_byte_year: DEFAULT_LAMPORTS_PER_BYTE_YEAR,
            exemption_threshold: DEFAULT_EXEMPTION_THRESHOLD,
            burn_percent: DEFAULT_BURN_PERCENT,
        }
    }
}

impl Rent {
    /// calculate how much rent to burn from the collected rent
    pub fn calculate_burn(&self, rent_collected: u64) -> (u64, u64) {
        let burned_portion = (rent_collected * u64::from(self.burn_percent)) / 100;
        (burned_portion, rent_collected - burned_portion)
    }
    /// minimum balance due for rent-exemption of a given size Account::data.len()
    ///
    /// Note: a stripped-down version of this calculation is used in
    /// calculate_split_rent_exempt_reserve in the stake program. When this function is updated, --
    /// eg. when making rent variable -- the stake program will need to be refactored
    pub fn minimum_balance(&self, data_len: usize) -> u64 {
        let bytes = data_len as u64;
        (((ACCOUNT_STORAGE_OVERHEAD + bytes) * self.lamports_per_byte_year) as f64
            * self.exemption_threshold) as u64
    }

    /// whether a given balance and data_len would be exempt
    pub fn is_exempt(&self, balance: u64, data_len: usize) -> bool {
        balance >= self.minimum_balance(data_len)
    }

    /// rent due on account's data_len with balance
    pub fn due(&self, balance: u64, data_len: usize, years_elapsed: f64) -> RentDue {
        if self.is_exempt(balance, data_len) {
            RentDue::Exempt
        } else {
            let actual_data_len = data_len as u64 + ACCOUNT_STORAGE_OVERHEAD;
            let lamports_per_year = self.lamports_per_byte_year * actual_data_len;
            RentDue::Paying((lamports_per_year as f64 * years_elapsed) as u64)
        }
    }

    pub fn free() -> Self {
        Self {
            lamports_per_byte_year: 0,
            ..Rent::default()
        }
    }

    pub fn with_slots_per_epoch(slots_per_epoch: u64) -> Self {
        let ratio = slots_per_epoch as f64 / DEFAULT_SLOTS_PER_EPOCH as f64;
        let exemption_threshold = DEFAULT_EXEMPTION_THRESHOLD as f64 * ratio;
        let lamports_per_byte_year = (DEFAULT_LAMPORTS_PER_BYTE_YEAR as f64 / ratio) as u64;
        Self {
            lamports_per_byte_year,
            exemption_threshold,
            ..Self::default()
        }
    }
}

/// Enumerate return values from `Rent::due()`
#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum RentDue {
    /// Used to indicate the account is rent exempt
    Exempt,
    /// The account owes rent, and the amount is the field
    Paying(u64),
}

impl RentDue {
    /// Return the lamports due for rent
    pub fn lamports(&self) -> u64 {
        match self {
            RentDue::Exempt => 0,
            RentDue::Paying(x) => *x,
        }
    }

    /// Return 'true' if rent exempt
    pub fn is_exempt(&self) -> bool {
        match self {
            RentDue::Exempt => true,
            RentDue::Paying(_) => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_due() {
        let default_rent = Rent::default();

        assert_eq!(
            default_rent.due(0, 2, 1.2),
            RentDue::Paying(
                (((2 + ACCOUNT_STORAGE_OVERHEAD) * DEFAULT_LAMPORTS_PER_BYTE_YEAR) as f64 * 1.2)
                    as u64
            ),
        );
        assert_eq!(
            default_rent.due(
                (((2 + ACCOUNT_STORAGE_OVERHEAD) * DEFAULT_LAMPORTS_PER_BYTE_YEAR) as f64
                    * DEFAULT_EXEMPTION_THRESHOLD) as u64,
                2,
                1.2
            ),
            RentDue::Exempt,
        );

        let custom_rent = Rent {
            lamports_per_byte_year: 5,
            exemption_threshold: 2.5,
            ..Rent::default()
        };

        assert_eq!(
            custom_rent.due(0, 2, 1.2),
            RentDue::Paying(
                (((2 + ACCOUNT_STORAGE_OVERHEAD) * custom_rent.lamports_per_byte_year) as f64 * 1.2)
                    as u64,
            )
        );

        assert_eq!(
            custom_rent.due(
                (((2 + ACCOUNT_STORAGE_OVERHEAD) * custom_rent.lamports_per_byte_year) as f64
                    * custom_rent.exemption_threshold) as u64,
                2,
                1.2
            ),
            RentDue::Exempt
        );
    }

    #[test]
    fn test_rent_due_lamports() {
        assert_eq!(RentDue::Exempt.lamports(), 0);

        let amount = 123;
        assert_eq!(RentDue::Paying(amount).lamports(), amount);
    }

    #[test]
    fn test_rent_due_is_exempt() {
        assert!(RentDue::Exempt.is_exempt());
        assert!(!RentDue::Paying(0).is_exempt());
    }
}
