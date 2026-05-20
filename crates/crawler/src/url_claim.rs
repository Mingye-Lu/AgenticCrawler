//! Atomic URL/scope claim registry.
//!
//! Two sibling sub-agents cannot crawl overlapping URLs. Before the fork
//! supervisor spawns a child, it calls [`UrlClaimRegistry::try_claim`] with
//! the child's [`CrawlScope`]; on success the registry returns a
//! [`ClaimGuard`] that owns the lifetime of the claim — when the guard
//! drops (child finishes, cancelled, or supervisor aborts setup), the
//! claim is released and another sibling may claim the same scope.
//!
//! Overlap policy:
//! - exact-vs-exact: same URL string fails the second claim.
//! - pattern-vs-pattern: identical regex source fails (conservative; subtly
//!   different but semantically overlapping regexes are an accepted
//!   footgun).
//! - exact-vs-pattern: an exact URL conflicts with any *already-claimed*
//!   pattern that matches it, and a pattern conflicts with any
//!   already-claimed exact URL it would match.

use std::sync::{Arc, Mutex};

use regex::Regex;

use crate::tool_effect::CrawlScope;

/// Reason a claim was rejected. Surfaced to the LLM so it can adjust scope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClaimConflict {
    /// Exact URL already claimed by another owner.
    ExactUrl { url: String, owner: String },
    /// Same regex pattern already claimed.
    Pattern { regex: String, owner: String },
    /// A pattern matched an already-claimed exact URL, or vice versa.
    PatternMatchesExact {
        regex: String,
        url: String,
        owner: String,
    },
    /// Pattern was syntactically invalid.
    InvalidRegex { regex: String, error: String },
}

impl std::fmt::Display for ClaimConflict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ExactUrl { url, owner } => {
                write!(f, "url `{url}` already claimed by sub-agent `{owner}`")
            }
            Self::Pattern { regex, owner } => write!(
                f,
                "pattern `{regex}` already claimed by sub-agent `{owner}`"
            ),
            Self::PatternMatchesExact { regex, url, owner } => write!(
                f,
                "pattern `{regex}` overlaps url `{url}` already claimed by sub-agent `{owner}`"
            ),
            Self::InvalidRegex { regex, error } => {
                write!(f, "invalid regex `{regex}`: {error}")
            }
        }
    }
}

impl std::error::Error for ClaimConflict {}

#[derive(Debug)]
enum Entry {
    Exact {
        url: String,
        owner: String,
    },
    Pattern {
        regex: Regex,
        source: String,
        owner: String,
    },
}

#[derive(Default)]
struct Inner {
    entries: Vec<Entry>,
}

/// Concurrent, atomic registry of in-flight URL/scope claims.
#[derive(Default, Clone)]
pub struct UrlClaimRegistry {
    inner: Arc<Mutex<Inner>>,
}

impl UrlClaimRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Attempt to claim `scope` on behalf of `owner_id`. On success returns a
    /// [`ClaimGuard`]; dropping the guard releases the claim. On conflict
    /// returns the specific [`ClaimConflict`] reason.
    ///
    /// Atomic: the conflict check and registration happen under the same
    /// lock, so two racing siblings cannot both succeed.
    ///
    /// # Errors
    /// See [`ClaimConflict`].
    pub fn try_claim(
        &self,
        scope: &CrawlScope,
        owner_id: &str,
    ) -> Result<ClaimGuard, ClaimConflict> {
        let mut guard = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        match scope {
            CrawlScope::SinglePage { url } => {
                check_and_insert_exact(&mut guard, url, owner_id)?;
                Ok(ClaimGuard {
                    registry: Arc::clone(&self.inner),
                    keys: vec![ClaimKey::Exact(url.clone())],
                })
            }
            CrawlScope::UrlList { urls } => {
                // All-or-nothing: validate every URL before inserting any.
                for url in urls {
                    check_exact(&guard, url, owner_id)?;
                }
                // Deduplicate within the submitted list; the LLM may
                // accidentally list the same URL twice.
                let mut seen = std::collections::HashSet::new();
                let mut keys = Vec::new();
                for url in urls {
                    if seen.insert(url.as_str()) {
                        guard.entries.push(Entry::Exact {
                            url: url.clone(),
                            owner: owner_id.to_string(),
                        });
                        keys.push(ClaimKey::Exact(url.clone()));
                    }
                }
                Ok(ClaimGuard {
                    registry: Arc::clone(&self.inner),
                    keys,
                })
            }
            CrawlScope::UrlPattern { regex } => {
                let compiled = Regex::new(regex).map_err(|error| ClaimConflict::InvalidRegex {
                    regex: regex.clone(),
                    error: error.to_string(),
                })?;
                check_pattern(&guard, regex, &compiled, owner_id)?;
                guard.entries.push(Entry::Pattern {
                    regex: compiled,
                    source: regex.clone(),
                    owner: owner_id.to_string(),
                });
                Ok(ClaimGuard {
                    registry: Arc::clone(&self.inner),
                    keys: vec![ClaimKey::Pattern(regex.clone())],
                })
            }
        }
    }

    /// Snapshot of currently claimed entries; intended for tests/debug.
    #[must_use]
    pub fn len(&self) -> usize {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entries
            .len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

fn check_exact(inner: &Inner, url: &str, _owner_id: &str) -> Result<(), ClaimConflict> {
    for entry in &inner.entries {
        match entry {
            Entry::Exact {
                url: claimed,
                owner,
            } if claimed == url => {
                return Err(ClaimConflict::ExactUrl {
                    url: url.to_string(),
                    owner: owner.clone(),
                });
            }
            Entry::Pattern {
                regex,
                source,
                owner,
            } if regex.is_match(url) => {
                return Err(ClaimConflict::PatternMatchesExact {
                    regex: source.clone(),
                    url: url.to_string(),
                    owner: owner.clone(),
                });
            }
            _ => {}
        }
    }
    Ok(())
}

fn check_and_insert_exact(
    inner: &mut Inner,
    url: &str,
    owner_id: &str,
) -> Result<(), ClaimConflict> {
    check_exact(inner, url, owner_id)?;
    inner.entries.push(Entry::Exact {
        url: url.to_string(),
        owner: owner_id.to_string(),
    });
    Ok(())
}

fn check_pattern(
    inner: &Inner,
    source: &str,
    compiled: &Regex,
    _owner_id: &str,
) -> Result<(), ClaimConflict> {
    for entry in &inner.entries {
        match entry {
            Entry::Pattern {
                source: existing,
                owner,
                ..
            } if existing == source => {
                return Err(ClaimConflict::Pattern {
                    regex: source.to_string(),
                    owner: owner.clone(),
                });
            }
            Entry::Exact { url, owner } if compiled.is_match(url) => {
                return Err(ClaimConflict::PatternMatchesExact {
                    regex: source.to_string(),
                    url: url.clone(),
                    owner: owner.clone(),
                });
            }
            _ => {}
        }
    }
    Ok(())
}

#[derive(Debug, Clone)]
enum ClaimKey {
    Exact(String),
    Pattern(String),
}

/// RAII handle that releases its claim(s) when dropped.
pub struct ClaimGuard {
    registry: Arc<Mutex<Inner>>,
    keys: Vec<ClaimKey>,
}

impl Drop for ClaimGuard {
    fn drop(&mut self) {
        let mut guard = self
            .registry
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard.entries.retain(|entry| match entry {
            Entry::Exact { url, .. } => !self
                .keys
                .iter()
                .any(|k| matches!(k, ClaimKey::Exact(claimed) if claimed == url)),
            Entry::Pattern { source, .. } => !self
                .keys
                .iter()
                .any(|k| matches!(k, ClaimKey::Pattern(claimed) if claimed == source)),
        });
    }
}

impl std::fmt::Debug for ClaimGuard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClaimGuard")
            .field("keys", &self.keys)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_page_exact_claim_succeeds_then_blocks_duplicate() {
        let registry = UrlClaimRegistry::new();
        let _g = registry
            .try_claim(
                &CrawlScope::SinglePage {
                    url: "https://example.com/a".to_string(),
                },
                "child-1",
            )
            .expect("first claim should succeed");

        let err = registry
            .try_claim(
                &CrawlScope::SinglePage {
                    url: "https://example.com/a".to_string(),
                },
                "child-2",
            )
            .expect_err("duplicate claim should fail");
        assert!(matches!(
            err,
            ClaimConflict::ExactUrl { ref url, ref owner }
                if url == "https://example.com/a" && owner == "child-1"
        ));
    }

    #[test]
    fn dropping_guard_releases_claim() {
        let registry = UrlClaimRegistry::new();
        let guard = registry
            .try_claim(
                &CrawlScope::SinglePage {
                    url: "https://example.com/a".to_string(),
                },
                "child-1",
            )
            .unwrap();
        drop(guard);

        registry
            .try_claim(
                &CrawlScope::SinglePage {
                    url: "https://example.com/a".to_string(),
                },
                "child-2",
            )
            .expect("re-claim after drop should succeed");
    }

    #[test]
    fn url_list_is_all_or_nothing() {
        let registry = UrlClaimRegistry::new();
        let _g1 = registry
            .try_claim(
                &CrawlScope::SinglePage {
                    url: "https://example.com/x".to_string(),
                },
                "child-1",
            )
            .unwrap();
        let err = registry
            .try_claim(
                &CrawlScope::UrlList {
                    urls: vec![
                        "https://example.com/y".to_string(),
                        "https://example.com/x".to_string(),
                    ],
                },
                "child-2",
            )
            .expect_err("UrlList overlap should fail");
        assert!(matches!(err, ClaimConflict::ExactUrl { .. }));
        // Verify no partial insertion: y is still claimable.
        registry
            .try_claim(
                &CrawlScope::SinglePage {
                    url: "https://example.com/y".to_string(),
                },
                "child-3",
            )
            .expect("y should not have been partially claimed");
    }

    #[test]
    fn pattern_conflicts_with_exact_url() {
        let registry = UrlClaimRegistry::new();
        let _exact = registry
            .try_claim(
                &CrawlScope::SinglePage {
                    url: "https://example.com/posts/42".to_string(),
                },
                "child-1",
            )
            .unwrap();
        let err = registry
            .try_claim(
                &CrawlScope::UrlPattern {
                    regex: r"^https://example\.com/posts/.*".to_string(),
                },
                "child-2",
            )
            .expect_err("pattern matching claimed exact should fail");
        assert!(matches!(err, ClaimConflict::PatternMatchesExact { .. }));
    }

    #[test]
    fn exact_url_conflicts_with_existing_pattern() {
        let registry = UrlClaimRegistry::new();
        let _pat = registry
            .try_claim(
                &CrawlScope::UrlPattern {
                    regex: r"^https://example\.com/posts/.*".to_string(),
                },
                "child-1",
            )
            .unwrap();
        let err = registry
            .try_claim(
                &CrawlScope::SinglePage {
                    url: "https://example.com/posts/99".to_string(),
                },
                "child-2",
            )
            .expect_err("exact URL inside claimed pattern should fail");
        assert!(matches!(err, ClaimConflict::PatternMatchesExact { .. }));
    }

    #[test]
    fn identical_pattern_conflicts() {
        let registry = UrlClaimRegistry::new();
        let _p1 = registry
            .try_claim(
                &CrawlScope::UrlPattern {
                    regex: r"^https://example\.com/x/.*".to_string(),
                },
                "child-1",
            )
            .unwrap();
        let err = registry
            .try_claim(
                &CrawlScope::UrlPattern {
                    regex: r"^https://example\.com/x/.*".to_string(),
                },
                "child-2",
            )
            .expect_err("duplicate pattern should fail");
        assert!(matches!(err, ClaimConflict::Pattern { .. }));
    }

    #[test]
    fn invalid_pattern_returns_invalid_regex_conflict() {
        let registry = UrlClaimRegistry::new();
        let err = registry
            .try_claim(
                &CrawlScope::UrlPattern {
                    regex: "[broken".to_string(),
                },
                "child-1",
            )
            .expect_err("invalid regex should fail");
        assert!(matches!(err, ClaimConflict::InvalidRegex { .. }));
    }

    #[test]
    fn url_list_deduplicates_intra_list_duplicate_urls() {
        let registry = UrlClaimRegistry::new();
        let _g = registry
            .try_claim(
                &CrawlScope::UrlList {
                    urls: vec![
                        "https://example.com/a".to_string(),
                        "https://example.com/b".to_string(),
                        "https://example.com/a".to_string(), // duplicate
                    ],
                },
                "child-1",
            )
            .expect("claim with intra-list duplicate should succeed (deduped)");

        // Only 2 entries registered, not 3.
        assert_eq!(registry.len(), 2);

        // The deduplicated URL is still claimable by another child after the
        // guard drops (this verifies len() tracked correctly).
        drop(_g);
        assert_eq!(registry.len(), 0);
    }

    #[test]
    fn racing_claims_only_one_wins() {
        use std::sync::Arc;
        use std::thread;

        let registry = Arc::new(UrlClaimRegistry::new());
        let mut handles = Vec::new();
        for i in 0..50 {
            let reg = Arc::clone(&registry);
            handles.push(thread::spawn(move || {
                reg.try_claim(
                    &CrawlScope::SinglePage {
                        url: "https://example.com/race".to_string(),
                    },
                    &format!("child-{i}"),
                )
                .ok()
            }));
        }
        // Collect ALL results first so the winning thread's ClaimGuard stays
        // alive while the late-joining threads finish their own try_claim
        // calls. Counting + dropping inside a single iterator chain would
        // free the entry between iterations and let late losers re-claim.
        let results: Vec<Option<ClaimGuard>> =
            handles.into_iter().map(|h| h.join().unwrap()).collect();
        let successes = results.iter().filter(|c| c.is_some()).count();
        assert_eq!(successes, 1);
    }
}
