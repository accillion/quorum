//! Spend caps (AC 250). A run needs both `--max-credits` (Lippa) and
//! `--max-usd` (direct vendor, extraction, clustering and judge calls).
//!
//! Before every paid call the caller reserves the worst case. A
//! reservation that would take committed spend (settled plus open
//! reservations) past either cap is refused, so the run stops *before* the
//! call. After the call the reservation is settled:
//! - with the billed amount when it is known;
//! - with the full reservation when billing is uncertain (a timeout, a
//!   dropped connection, a 5xx): uncertain counts as spent;
//! - with zero only when the request provably never left this process.

use std::collections::BTreeMap;

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Unit {
    /// Lippa credits.
    Credits,
    /// Micro-USD.
    MicroUsd,
}

#[derive(thiserror::Error, Debug, PartialEq, Eq)]
pub enum BudgetError {
    #[error("stopping: reserving {requested} {unit:?} would take committed spend to {would_be}, over the cap of {cap}")]
    WouldExceed {
        unit: Unit,
        cap: u64,
        requested: u64,
        would_be: u64,
    },
    #[error("unknown reservation {0}")]
    UnknownReservation(u64),
}

/// How a paid call ended, for settling its reservation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Settle {
    /// The billed amount, from the response's usage or the session detail.
    Billed(u64),
    /// The outcome is unknown; the whole reservation counts as spent.
    Uncertain,
    /// The request never left this process (for example, the connection
    /// was refused). Nothing is charged.
    NeverSent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Reservation {
    id: u64,
    unit: Unit,
    amount: u64,
}

impl Reservation {
    pub fn amount(&self) -> u64 {
        self.amount
    }
}

#[derive(Debug, Clone, Default, Serialize)]
struct Pot {
    cap: u64,
    spent: u64,
    reserved: u64,
    /// Billed amounts above their reservation (an estimate was low).
    overshoot: u64,
}

impl Pot {
    fn committed(&self) -> u64 {
        self.spent + self.reserved
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Budget {
    credits: Pot,
    usd: Pot,
    #[serde(skip)]
    open: BTreeMap<u64, Reservation>,
    #[serde(skip)]
    next: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Snapshot {
    pub credits_cap: u64,
    pub credits_spent: u64,
    pub credits_reserved: u64,
    pub usd_cap_micros: u64,
    pub usd_spent_micros: u64,
    pub usd_reserved_micros: u64,
    pub usd_overshoot_micros: u64,
}

impl Budget {
    pub fn new(max_credits: u64, max_usd_micros: u64) -> Self {
        Self {
            credits: Pot {
                cap: max_credits,
                ..Default::default()
            },
            usd: Pot {
                cap: max_usd_micros,
                ..Default::default()
            },
            open: BTreeMap::new(),
            next: 1,
        }
    }

    fn pot(&mut self, unit: Unit) -> &mut Pot {
        match unit {
            Unit::Credits => &mut self.credits,
            Unit::MicroUsd => &mut self.usd,
        }
    }

    pub fn reserve(&mut self, unit: Unit, amount: u64) -> Result<Reservation, BudgetError> {
        let pot = self.pot(unit);
        let would_be = pot.committed().saturating_add(amount);
        if would_be > pot.cap {
            return Err(BudgetError::WouldExceed {
                unit,
                cap: pot.cap,
                requested: amount,
                would_be,
            });
        }
        pot.reserved += amount;
        let r = Reservation {
            id: self.next,
            unit,
            amount,
        };
        self.next += 1;
        self.open.insert(r.id, r);
        Ok(r)
    }

    pub fn settle(&mut self, r: Reservation, how: Settle) -> Result<u64, BudgetError> {
        let r = self
            .open
            .remove(&r.id)
            .ok_or(BudgetError::UnknownReservation(r.id))?;
        let charged = match how {
            Settle::Billed(n) => n,
            Settle::Uncertain => r.amount,
            Settle::NeverSent => 0,
        };
        let pot = self.pot(r.unit);
        pot.reserved -= r.amount;
        pot.spent += charged;
        pot.overshoot += charged.saturating_sub(r.amount);
        Ok(charged)
    }

    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            credits_cap: self.credits.cap,
            credits_spent: self.credits.spent,
            credits_reserved: self.credits.reserved,
            usd_cap_micros: self.usd.cap,
            usd_spent_micros: self.usd.spent,
            usd_reserved_micros: self.usd.reserved,
            usd_overshoot_micros: self.usd.overshoot,
        }
    }
}

/// Parse a `--max-usd` value such as `300` or `12.50` into micro-USD.
pub fn parse_usd(s: &str) -> Result<u64, String> {
    let s = s.trim().trim_start_matches('$');
    let (whole, frac) = s.split_once('.').unwrap_or((s, ""));
    if whole.is_empty()
        || !whole.bytes().all(|b| b.is_ascii_digit())
        || !frac.bytes().all(|b| b.is_ascii_digit())
        || frac.len() > 6
    {
        return Err(format!("`{s}` is not a USD amount like 300 or 12.50"));
    }
    let w: u64 = whole.parse().map_err(|_| format!("`{s}` is too large"))?;
    let f: u64 = format!("{frac:0<6}").parse().unwrap_or(0);
    w.checked_mul(1_000_000)
        .and_then(|m| m.checked_add(f))
        .ok_or_else(|| format!("`{s}` is too large"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn negative_control_a_reservation_past_the_cap_is_refused() {
        let mut b = Budget::new(10, 1_000);
        let e = b.reserve(Unit::MicroUsd, 1_001).unwrap_err();
        assert_eq!(
            e,
            BudgetError::WouldExceed {
                unit: Unit::MicroUsd,
                cap: 1_000,
                requested: 1_001,
                would_be: 1_001
            }
        );
        assert_eq!(b.snapshot().usd_reserved_micros, 0);
    }

    /// A run of identical calls stops before the call that could exceed
    /// either cap, never after it.
    #[test]
    fn a_run_stops_before_exceeding_either_cap() {
        for (credits_cap, usd_cap) in [(9, 1_000_000), (100, 250_000)] {
            let mut b = Budget::new(credits_cap, usd_cap);
            let mut sessions = 0;
            let mut direct = 0;
            // One Lippa session (worst case 3 credits) and one direct call
            // (worst case 60,000 µ$, billed 40,000 µ$) per iteration.
            while let Ok(rc) = b.reserve(Unit::Credits, 3) {
                b.settle(rc, Settle::Billed(3)).unwrap();
                sessions += 1;
                let Ok(ru) = b.reserve(Unit::MicroUsd, 60_000) else {
                    break;
                };
                b.settle(ru, Settle::Billed(40_000)).unwrap();
                direct += 1;
                assert!(b.snapshot().credits_spent <= credits_cap);
                assert!(b.snapshot().usd_spent_micros <= usd_cap);
            }
            let s = b.snapshot();
            assert!(s.credits_spent <= credits_cap && s.usd_spent_micros <= usd_cap);
            if credits_cap == 9 {
                assert_eq!(
                    (sessions, direct),
                    (3, 3),
                    "credits bind after three sessions"
                );
            } else {
                // 250,000: after 5 calls 200,000 spent; a 6th needs 260,000.
                assert_eq!(direct, 5);
                assert_eq!(s.usd_spent_micros, 200_000);
            }
        }
    }

    #[test]
    fn uncertain_billing_counts_the_whole_reservation_as_spent() {
        let mut b = Budget::new(0, 100);
        let r = b.reserve(Unit::MicroUsd, 70).unwrap();
        assert_eq!(b.settle(r, Settle::Uncertain).unwrap(), 70);
        assert!(b.reserve(Unit::MicroUsd, 31).is_err());
        assert!(b.reserve(Unit::MicroUsd, 30).is_ok());
    }

    #[test]
    fn open_reservations_count_against_the_cap() {
        let mut b = Budget::new(0, 100);
        let _r1 = b.reserve(Unit::MicroUsd, 60).unwrap();
        assert!(b.reserve(Unit::MicroUsd, 41).is_err());
    }

    #[test]
    fn never_sent_releases_and_double_settle_is_refused() {
        let mut b = Budget::new(0, 100);
        let r = b.reserve(Unit::MicroUsd, 100).unwrap();
        assert_eq!(b.settle(r, Settle::NeverSent).unwrap(), 0);
        assert_eq!(
            b.settle(r, Settle::NeverSent),
            Err(BudgetError::UnknownReservation(1))
        );
        assert!(b.reserve(Unit::MicroUsd, 100).is_ok());
    }

    #[test]
    fn overshoot_is_recorded() {
        let mut b = Budget::new(0, 1_000);
        let r = b.reserve(Unit::MicroUsd, 10).unwrap();
        b.settle(r, Settle::Billed(15)).unwrap();
        assert_eq!(b.snapshot().usd_overshoot_micros, 5);
    }

    #[test]
    fn usd_parsing() {
        assert_eq!(parse_usd("300").unwrap(), 300_000_000);
        assert_eq!(parse_usd("$12.5").unwrap(), 12_500_000);
        assert_eq!(parse_usd("0.000001").unwrap(), 1);
        assert!(parse_usd("1.0000001").is_err());
        assert!(parse_usd("-1").is_err());
        assert!(parse_usd("abc").is_err());
    }
}
