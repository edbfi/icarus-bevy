//! Bounded actor UID ownership, independently implemented from behavior notes.
//!
//! Availability treats every `i32`, including zero and -1, equally. A selected
//! -1 is nevertheless the failure sentinel and is never claimed. Candidate
//! increments wrap at the signed 32-bit boundary deliberately. This defines
//! overflow rather than relying on the original runtime's integer behavior.
use std::collections::{BTreeMap, BTreeSet};

/// Native policy: each of claims, reservations and remaps holds at most this
/// many entries. Indexes contain at most the same number of entries. Together
/// these bounds limit storage; they are not an exact allocator/RSS guarantee.
pub const MAX_UID_ENTRIES: usize = 262_144;
pub const RANDOM_ATTEMPTS: usize = 1_000;
pub const SEQUENTIAL_ATTEMPTS: usize = 100_000;
/// Authored substitute for an engine object index, reset with each native world.
pub const FIRST_NATIVE_OBJECT_ID: i32 = 65_536;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActorUidError {
    ClaimedLimit,
    ReservationLimit,
    RemapLimit,
    InvalidLimit,
    /// Search exhausted, or the selected candidate was the -1 sentinel.
    Unavailable,
}

impl std::fmt::Display for ActorUidError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::ClaimedLimit => "actor UID claimed-entry limit reached",
            Self::ReservationLimit => "actor UID reservation-entry limit reached",
            Self::RemapLimit => "actor UID remap-entry limit reached",
            Self::InvalidLimit => "actor UID entry limit exceeds native policy",
            Self::Unavailable => "no actor UID granted",
        })
    }
}

impl std::error::Error for ActorUidError {}

/// Authored deterministic xorshift64 PRNG, **not** MSVC/C-runtime `rand`.
/// Zero seeds use a fixed nonzero state; output uses the low fifteen bits.
pub fn seeded_random(seed: u64) -> impl FnMut() -> u16 + Send + Sync {
    let mut state = if seed == 0 {
        0x9e37_79b9_7f4a_7c15
    } else {
        seed
    };
    move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        (state & 0x7fff) as u16
    }
}

/// Ordered claims and remaps, requester reservations, and bounded candidate
/// search. Requesters are native caller-owned identities, never engine pointers.
/// Resource-policy errors leave all stored entries unchanged. The injected
/// random source must itself return promptly; its output is masked to 15 bits.
pub struct ActorUidLibrary {
    claimed: Vec<i32>,
    claimed_index: BTreeSet<i32>,
    reservations: BTreeMap<u64, i32>,
    reserved_counts: BTreeMap<i32, usize>,
    remaps: Vec<(i32, i32)>,
    limit: usize,
    random: Box<dyn FnMut() -> u16 + Send + Sync>,
}

impl ActorUidLibrary {
    pub fn new(seed: u64) -> Self {
        Self::with_random(seeded_random(seed))
    }

    pub fn with_random(random: impl FnMut() -> u16 + Send + Sync + 'static) -> Self {
        Self::with_limit(MAX_UID_ENTRIES, random).expect("the default limit is valid")
    }

    /// Smaller limits support deployments/tests; larger ones are rejected.
    pub fn with_limit(
        limit: usize,
        random: impl FnMut() -> u16 + Send + Sync + 'static,
    ) -> Result<Self, ActorUidError> {
        if limit > MAX_UID_ENTRIES {
            return Err(ActorUidError::InvalidLimit);
        }
        Ok(Self {
            claimed: Vec::new(),
            claimed_index: BTreeSet::new(),
            reservations: BTreeMap::new(),
            reserved_counts: BTreeMap::new(),
            remaps: Vec::new(),
            limit,
            random: Box::new(random),
        })
    }

    pub fn claimed(&self) -> &[i32] {
        &self.claimed
    }

    pub fn reservations(&self) -> &BTreeMap<u64, i32> {
        &self.reservations
    }

    pub fn remaps(&self) -> &[(i32, i32)] {
        &self.remaps
    }

    /// Whether `uid` is currently claimed (by a pawn or a saved seed).
    pub fn is_claimed(&self, uid: i32) -> bool {
        self.claimed_index.contains(&uid)
    }

    pub fn available(&self, uid: i32, requester: Option<u64>) -> bool {
        if self.claimed_index.contains(&uid) {
            return false;
        }
        if requester.is_some_and(|who| self.reservations.get(&who) == Some(&uid)) {
            return true;
        }
        !self.reserved_counts.contains_key(&uid)
    }

    /// Bulk add/replace desired UIDs. Duplicate requesters use their final
    /// supplied value; multiple requesters may reserve the same UID. Over-cap
    /// batches reject atomically, with at most `limit` entries of staging.
    pub fn add_desired_uids(
        &mut self,
        desired: impl IntoIterator<Item = (u64, i32)>,
    ) -> Result<(), ActorUidError> {
        let mut next = self.reservations.clone();
        for (requester, uid) in desired {
            if !next.contains_key(&requester) && next.len() == self.limit {
                return Err(ActorUidError::ReservationLimit);
            }
            next.insert(requester, uid);
        }
        let mut counts = BTreeMap::new();
        for uid in next.values() {
            *counts.entry(*uid).or_insert(0) += 1;
        }
        self.reservations = next;
        self.reserved_counts = counts;
        Ok(())
    }

    /// Try desired, its wrapping successor, random draws, then successors of
    /// the last draw. A selected/final failure sentinel records `(desired, -1)`
    /// when different from desired, but never becomes a claim. Storage-limit
    /// errors neither claim nor record a remap.
    pub fn try_claim(
        &mut self,
        desired: i32,
        requester: Option<u64>,
    ) -> Result<i32, ActorUidError> {
        // Reaching the storage bound refuses new work even if search would
        // eventually select -1. Never silently discard an ownership record.
        if self.claimed.len() == self.limit {
            return Err(ActorUidError::ClaimedLimit);
        }
        let mut grant = None;
        for candidate in [desired, desired.wrapping_add(1)] {
            if self.available(candidate, requester) {
                grant = Some(candidate);
                break;
            }
        }
        let mut last = 0;
        if grant.is_none() {
            for _ in 0..RANDOM_ATTEMPTS {
                last = i32::from((self.random)() & 0x7fff);
                if self.available(last, requester) {
                    grant = Some(last);
                    break;
                }
            }
        }
        if grant.is_none() {
            for _ in 0..SEQUENTIAL_ATTEMPTS {
                last = last.wrapping_add(1);
                if self.available(last, requester) {
                    grant = Some(last);
                    break;
                }
            }
        }
        let grant = grant.unwrap_or(-1);
        if grant != desired {
            if self.remaps.len() == self.limit {
                return Err(ActorUidError::RemapLimit);
            }
            self.remaps.push((desired, grant));
        }
        if grant == -1 {
            return Err(ActorUidError::Unavailable);
        }
        self.claimed.push(grant);
        self.claimed_index.insert(grant);
        Ok(grant)
    }

    /// Free ownership only: reservations and historical remaps survive.
    pub fn free(&mut self, uid: i32) -> bool {
        if !self.claimed_index.remove(&uid) {
            return false;
        }
        self.claimed.retain(|claimed| *claimed != uid);
        true
    }

    /// Seed the occupied namespace from saved actor records, deduplicating
    /// repeated references. Positive UIDs only; no restoration or import is
    /// implied. Each new UID is claimed unchanged with its own native requester.
    /// Preflight makes an over-cap batch atomic. Call before joining peers.
    pub fn seed_saved(&mut self, uids: impl IntoIterator<Item = i32>) -> Result<(), ActorUidError> {
        let mut added = Vec::new();
        let mut seen = BTreeSet::new();
        for uid in uids {
            if uid > 0 && !self.claimed_index.contains(&uid) && !seen.contains(&uid) {
                if self.claimed.len() + added.len() == self.limit {
                    return Err(ActorUidError::ClaimedLimit);
                }
                seen.insert(uid);
                added.push(uid);
            }
        }
        // Saved seeding is only valid without reservations: otherwise a
        // restore could remap and lose the original occupied namespace.
        if !self.reservations.is_empty() {
            return Err(ActorUidError::Unavailable);
        }
        for uid in added {
            self.try_claim(uid, Some(uid as u64))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    #[test]
    fn desired_then_successor_preserve_claim_and_remap_order() {
        let mut library = ActorUidLibrary::with_random(|| panic!("unexpected random draw"));
        assert_eq!(library.try_claim(42, None), Ok(42));
        assert_eq!(library.try_claim(42, None), Ok(43));
        assert_eq!(library.claimed(), [42, 43]);
        assert_eq!(library.remaps(), [(42, 43)]);
    }

    #[test]
    fn both_taken_use_random_and_mask_injected_output() {
        let mut library = ActorUidLibrary::with_random(|| 0x8007);
        library.seed_saved([42, 43]).unwrap();
        assert_eq!(library.try_claim(42, None), Ok(7));
        assert_eq!(library.remaps(), [(42, 7)]);
    }

    #[test]
    fn random_exhaustion_uses_last_draw_then_sequential_success() {
        let draws = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&draws);
        let mut library = ActorUidLibrary::with_random(move || {
            let draw = counted.fetch_add(1, Ordering::Relaxed);
            if draw + 1 == RANDOM_ATTEMPTS { 20 } else { 7 }
        });
        library.seed_saved([7, 20, 42, 43]).unwrap();
        assert_eq!(library.try_claim(42, None), Ok(21));
        assert_eq!(draws.load(Ordering::Relaxed), RANDOM_ATTEMPTS);
        assert_eq!(library.remaps(), [(42, 21)]);
    }

    #[test]
    fn sequential_last_candidate_succeeds_and_total_exhaustion_fails() {
        let mut library = ActorUidLibrary::with_random(|| 0);
        // Zero is available to the general API, and can also be occupied.
        assert_eq!(library.try_claim(0, None), Ok(0));
        library.seed_saved(1..100_000).unwrap();
        library.seed_saved([200_000, 200_001]).unwrap();
        assert_eq!(library.try_claim(200_000, None), Ok(100_000));
        assert_eq!(
            library.try_claim(200_000, None),
            Err(ActorUidError::Unavailable)
        );
        assert_eq!(library.remaps(), [(200_000, 100_000), (200_000, -1)]);
        assert!(!library.claimed().contains(&-1));
        assert!(
            library.available(100_001, None),
            "search stops at its bound"
        );
    }

    #[test]
    fn reservations_allow_self_even_when_shared_but_never_override_claims() {
        let mut library = ActorUidLibrary::new(1);
        library
            .add_desired_uids([(1, 42), (2, 42), (3, -1), (4, 0)])
            .unwrap();
        assert!(!library.available(42, None));
        assert!(!library.available(42, Some(99)));
        assert!(library.available(42, Some(1)));
        assert!(library.available(42, Some(2)));
        assert!(!library.available(-1, None));
        assert!(library.available(-1, Some(3)));
        assert!(!library.available(0, None));
        assert!(library.available(0, Some(4)));
        assert_eq!(library.try_claim(42, Some(2)), Ok(42));
        assert!(!library.available(42, Some(1)));
        assert_eq!(library.try_claim(42, Some(1)), Ok(43));
        library
            .add_desired_uids([(1, 50), (2, 51), (2, 52)])
            .unwrap();
        library.free(42);
        assert!(
            library.available(42, None),
            "replaced reservations release old UID"
        );
        assert!(!library.available(50, None));
        assert!(!library.available(52, None));
        assert_eq!(library.reservations().get(&2), Some(&52));
    }

    #[test]
    fn free_keeps_order_reservations_and_remaps_and_allows_reclaim() {
        let mut library = ActorUidLibrary::with_random(|| 7);
        library.seed_saved([4, 5, 6]).unwrap();
        library.add_desired_uids([(1, 4)]).unwrap();
        assert!(library.free(5));
        assert!(!library.free(5));
        assert_eq!(library.claimed(), [4, 6]);
        assert_eq!(library.try_claim(5, None), Ok(5));
        assert_eq!(library.try_claim(4, None), Ok(7));
        assert!(library.free(4));
        assert!(
            !library.available(4, None),
            "free does not remove a reservation"
        );
        assert_eq!(library.remaps(), [(4, 7)]);
        assert_eq!(library.try_claim(4, Some(1)), Ok(4));
    }

    #[test]
    fn minus_one_is_available_but_signals_failure_and_increments_wrap() {
        let mut library = ActorUidLibrary::new(1);
        assert!(library.available(-1, None));
        assert_eq!(library.try_claim(-1, None), Err(ActorUidError::Unavailable));
        assert!(library.claimed().is_empty());
        assert_eq!(library.try_claim(-2, None), Ok(-2));
        assert_eq!(library.try_claim(-2, None), Err(ActorUidError::Unavailable));
        assert_eq!(library.remaps(), [(-2, -1)]);
        assert_eq!(library.try_claim(i32::MAX, None), Ok(i32::MAX));
        assert_eq!(library.try_claim(i32::MAX, None), Ok(i32::MIN));
    }

    #[test]
    fn claims_reservations_and_remaps_fail_closed_at_caps() {
        let mut library = ActorUidLibrary::with_limit(2, || 7).unwrap();
        library.seed_saved([1, 2]).unwrap();
        assert_eq!(library.try_claim(3, None), Err(ActorUidError::ClaimedLimit));
        assert_eq!(library.claimed(), [1, 2]);
        library.free(1);
        assert_eq!(library.seed_saved([3, 4]), Err(ActorUidError::ClaimedLimit));
        assert_eq!(library.claimed(), [2], "over-cap seeding is atomic");
        library.add_desired_uids([(1, 42), (2, 50)]).unwrap();
        assert_eq!(
            library.add_desired_uids([(1, 43), (3, 44)]),
            Err(ActorUidError::ReservationLimit)
        );
        assert_eq!(library.reservations().get(&1), Some(&42));
        assert_eq!(library.seed_saved([8]), Err(ActorUidError::Unavailable));
        for _ in 0..2 {
            assert_eq!(library.try_claim(42, None), Ok(43));
            library.free(43);
        }
        assert_eq!(library.try_claim(42, None), Err(ActorUidError::RemapLimit));
        assert_eq!(library.claimed(), [2], "failed remap must not claim");
        assert!(library.available(43, None));
        assert_eq!(library.remaps(), [(42, 43), (42, 43)]);
        assert!(ActorUidLibrary::with_limit(MAX_UID_ENTRIES + 1, || 0).is_err());
        let mut zero = ActorUidLibrary::with_limit(0, || 0).unwrap();
        assert_eq!(zero.try_claim(42, None), Err(ActorUidError::ClaimedLimit));
        assert_eq!(
            zero.add_desired_uids([(1, 42)]),
            Err(ActorUidError::ReservationLimit)
        );
    }

    #[test]
    fn seeded_random_is_reproducible_bounded_and_nonconstant() {
        for seed in [0, 1, u64::MAX] {
            let mut a = seeded_random(seed);
            let mut b = seeded_random(seed);
            let first = a();
            assert_eq!(first, b());
            let mut changed = false;
            for _ in 0..1_000 {
                let value = a();
                assert_eq!(value, b());
                assert!(value <= 32_767);
                changed |= value != first;
            }
            assert!(changed);
        }
    }

    #[test]
    fn saved_seed_is_positive_deduplicated_and_does_not_remap() {
        let mut library = ActorUidLibrary::with_random(|| 6);
        library.seed_saved([0, -1, -10, 4, 4, 3]).unwrap();
        library.seed_saved([3, 4, 5]).unwrap();
        assert_eq!(library.claimed(), [4, 3, 5]);
        assert!(library.remaps().is_empty());
        assert_eq!(library.try_claim(3, None), Ok(6));
    }
}
