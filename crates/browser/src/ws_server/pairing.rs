//! One-time pairing: hands the bridge token to the extension after the user
//! types a code that only the acrawl side displayed.
//!
//! The code travels acrawl -> user -> extension popup, never extension ->
//! acrawl, so a local process that merely reaches the port cannot approve its
//! own request. Wrong guesses burn the offer.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::Serialize;

use super::auth::constant_time_eq;

pub const PAIRING_TTL: Duration = Duration::from_secs(180);
const MAX_WRONG_CODES: u8 = 5;

/// Who is asking to pair, as shown in the extension popup. Self-reported, so
/// it is context for the user, not authentication.
#[derive(Debug, Clone, Serialize)]
pub struct PairingHost {
    pub client: String,
    pub mode: String,
    pub pid: u32,
    pub cwd: String,
}

impl PairingHost {
    #[must_use]
    pub fn current(client: impl Into<String>, mode: impl Into<String>) -> Self {
        Self {
            client: client.into(),
            mode: mode.into(),
            pid: std::process::id(),
            cwd: std::env::current_dir()
                .map(|p| p.display().to_string())
                .unwrap_or_default(),
        }
    }
}

/// What the acrawl side shows the user.
#[derive(Debug, Clone)]
pub struct PairingOffer {
    pub code: String,
    pub expires_in: Duration,
}

impl PairingOffer {
    /// `482 913`, with the lifetime so the user knows how long they have.
    #[must_use]
    pub fn describe(&self) -> String {
        format!(
            "{} {} (enter it in the acrawl Bridge extension popup; expires in {}s)",
            &self.code[..3],
            &self.code[3..],
            self.expires_in.as_secs()
        )
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum RedeemError {
    NoOffer,
    Wrong,
}

struct Offer {
    code: String,
    host: PairingHost,
    expires_at: Instant,
    wrong: u8,
}

#[derive(Default)]
pub(super) struct PairingState {
    offer: Mutex<Option<Offer>>,
}

impl PairingState {
    pub(super) fn open(&self, host: PairingHost, ttl: Duration) -> PairingOffer {
        use rand::Rng;
        let code = format!("{:06}", rand::thread_rng().gen_range(0..1_000_000u32));
        let offer = PairingOffer {
            code: code.clone(),
            expires_in: ttl,
        };
        *self.lock() = Some(Offer {
            code,
            host,
            expires_at: Instant::now() + ttl,
            wrong: 0,
        });
        offer
    }

    /// Host and seconds left for the live offer, never the code.
    pub(super) fn info(&self) -> Option<(PairingHost, u64)> {
        let mut guard = self.lock();
        let offer = guard.as_ref()?;
        if let Some(left) = offer.expires_at.checked_duration_since(Instant::now()) {
            return Some((offer.host.clone(), left.as_secs()));
        }
        *guard = None;
        None
    }

    pub(super) fn redeem(&self, code: &str) -> Result<(), RedeemError> {
        let mut guard = self.lock();
        let Some(offer) = guard.as_mut() else {
            return Err(RedeemError::NoOffer);
        };
        if Instant::now() >= offer.expires_at {
            *guard = None;
            return Err(RedeemError::NoOffer);
        }
        let digits: String = code.chars().filter(char::is_ascii_digit).collect();
        if constant_time_eq(digits.as_bytes(), offer.code.as_bytes()) {
            *guard = None;
            return Ok(());
        }
        offer.wrong += 1;
        if offer.wrong >= MAX_WRONG_CODES {
            *guard = None;
        }
        Err(RedeemError::Wrong)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Option<Offer>> {
        self.offer
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host() -> PairingHost {
        PairingHost::current("test", "repl")
    }

    #[test]
    fn right_code_redeems_once() {
        let s = PairingState::default();
        let offer = s.open(host(), PAIRING_TTL);
        assert_eq!(s.redeem(&offer.code), Ok(()));
        assert_eq!(s.redeem(&offer.code), Err(RedeemError::NoOffer));
    }

    #[test]
    fn code_accepts_display_spacing() {
        let s = PairingState::default();
        let offer = s.open(host(), PAIRING_TTL);
        let spaced = format!("{} {}", &offer.code[..3], &offer.code[3..]);
        assert_eq!(s.redeem(&spaced), Ok(()));
    }

    #[test]
    fn five_wrong_codes_burn_the_offer() {
        let s = PairingState::default();
        let offer = s.open(host(), PAIRING_TTL);
        let wrong = if offer.code == "000000" {
            "000001"
        } else {
            "000000"
        };
        for _ in 0..5 {
            assert_eq!(s.redeem(wrong), Err(RedeemError::Wrong));
        }
        assert_eq!(s.redeem(&offer.code), Err(RedeemError::NoOffer));
    }

    #[test]
    fn expired_offer_is_gone_and_info_hides_code() {
        let s = PairingState::default();
        let offer = s.open(host(), Duration::ZERO);
        assert!(s.info().is_none());
        assert_eq!(s.redeem(&offer.code), Err(RedeemError::NoOffer));
        let live = s.open(host(), PAIRING_TTL);
        let (h, left) = s.info().expect("live offer");
        assert_eq!(h.client, "test");
        assert!(left > 170);
        assert!(!format!("{h:?}").contains(&live.code));
    }
}
