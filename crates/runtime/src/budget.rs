use std::str::FromStr;
use std::sync::atomic::AtomicU64;
use std::sync::Arc;

const MILLICENTS_PER_USD: u64 = 100_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BudgetMode {
    Warn,
    Block,
    RouteDown,
}

impl BudgetMode {
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        Self::from_str(s).ok()
    }
}

impl FromStr for BudgetMode {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "warn" => Ok(Self::Warn),
            "block" => Ok(Self::Block),
            "route_down" => Ok(Self::RouteDown),
            _ => Err(()),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum BudgetDecision {
    Allow,
    Warn { remaining_usd: f64 },
    Block,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BudgetEnforcer {
    max_cost_usd: f64,
    mode: BudgetMode,
    warn_threshold_pct: u32,
}

impl BudgetEnforcer {
    #[must_use]
    pub fn new(max_cost_usd: f64, mode: BudgetMode, warn_threshold_pct: u32) -> Self {
        Self {
            max_cost_usd,
            mode,
            warn_threshold_pct,
        }
    }

    #[must_use]
    pub fn check(&self, current_cost_usd: f64) -> BudgetDecision {
        if current_cost_usd >= self.max_cost_usd {
            return match self.mode {
                BudgetMode::Warn => BudgetDecision::Warn { remaining_usd: 0.0 },
                BudgetMode::Block | BudgetMode::RouteDown => BudgetDecision::Block,
            };
        }

        let pct_used = (current_cost_usd / self.max_cost_usd) * 100.0;
        if pct_used >= f64::from(self.warn_threshold_pct) {
            BudgetDecision::Warn {
                remaining_usd: self.max_cost_usd - current_cost_usd,
            }
        } else {
            BudgetDecision::Allow
        }
    }
}

#[must_use]
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss
)]
pub fn usd_to_millicents(usd: f64) -> u64 {
    (usd * MILLICENTS_PER_USD as f64) as u64
}

#[must_use]
#[allow(clippy::cast_precision_loss)]
pub fn millicents_to_usd(millicents: u64) -> f64 {
    millicents as f64 / MILLICENTS_PER_USD as f64
}

pub type SharedCostCounter = Arc<AtomicU64>;

#[must_use]
pub fn new_cost_counter() -> SharedCostCounter {
    Arc::new(AtomicU64::new(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allow_below_threshold() {
        let enforcer = BudgetEnforcer::new(1.0, BudgetMode::Block, 80);
        assert_eq!(enforcer.check(0.5), BudgetDecision::Allow);
    }

    #[test]
    fn warn_at_threshold() {
        let enforcer = BudgetEnforcer::new(1.0, BudgetMode::Warn, 80);
        assert!(matches!(
            enforcer.check(0.85),
            BudgetDecision::Warn { remaining_usd } if remaining_usd > 0.0
        ));
    }

    #[test]
    fn block_at_or_above_limit() {
        let enforcer = BudgetEnforcer::new(1.0, BudgetMode::Block, 80);
        assert_eq!(enforcer.check(1.01), BudgetDecision::Block);
    }

    #[test]
    fn route_down_acts_like_block() {
        let enforcer = BudgetEnforcer::new(1.0, BudgetMode::RouteDown, 80);
        assert_eq!(enforcer.check(1.01), BudgetDecision::Block);
    }

    #[test]
    fn warn_mode_never_blocks() {
        let enforcer = BudgetEnforcer::new(1.0, BudgetMode::Warn, 80);
        assert_eq!(
            enforcer.check(2.0),
            BudgetDecision::Warn { remaining_usd: 0.0 }
        );
    }

    #[test]
    fn converts_between_usd_and_millicents() {
        let usd = 1.2345;
        assert_eq!(usd_to_millicents(usd), 123_450);
        assert!((millicents_to_usd(123_450) - usd).abs() < 1e-6);
    }
}
