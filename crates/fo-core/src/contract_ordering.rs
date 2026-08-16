use std::cmp::Ordering;

use crate::EconomicTermKind;

impl PartialOrd for EconomicTermKind {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for EconomicTermKind {
    fn cmp(&self, other: &Self) -> Ordering {
        economic_term_rank(*self).cmp(&economic_term_rank(*other))
    }
}

const fn economic_term_rank(kind: EconomicTermKind) -> u8 {
    match kind {
        EconomicTermKind::Money => 0,
        EconomicTermKind::Percentage => 1,
        EconomicTermKind::Duration => 2,
        EconomicTermKind::NoticePeriod => 3,
        EconomicTermKind::PaymentTerm => 4,
        EconomicTermKind::BaseRent => 5,
        EconomicTermKind::RentEscalation => 6,
        EconomicTermKind::PercentageRent => 7,
        EconomicTermKind::SecurityDeposit => 8,
        EconomicTermKind::TenantImprovementAllowance => 9,
        EconomicTermKind::LiabilityCap => 10,
        EconomicTermKind::InsuranceLimit => 11,
        EconomicTermKind::ServiceLevel => 12,
        EconomicTermKind::InterestRate => 13,
        EconomicTermKind::RenewalTerm => 14,
    }
}
