use std::collections::BTreeMap;

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use rand_core::{OsRng, RngCore};
use subtle::ConstantTimeEq;
use time::{Duration, OffsetDateTime};

const PLATFORM_SESSION_TTL: Duration = Duration::hours(8);

#[derive(Debug, Default)]
pub struct PlatformSessions {
    expires_at_by_token: BTreeMap<String, OffsetDateTime>,
}

impl PlatformSessions {
    pub fn issue(&mut self) -> String {
        self.prune();
        let mut token_bytes = [0_u8; 32];
        OsRng.fill_bytes(&mut token_bytes);
        let token = URL_SAFE_NO_PAD.encode(token_bytes);
        self.expires_at_by_token.insert(
            token.clone(),
            OffsetDateTime::now_utc() + PLATFORM_SESSION_TTL,
        );
        token
    }

    pub fn authenticate(&mut self, token: &str) -> bool {
        self.prune();
        self.expires_at_by_token.contains_key(token.trim())
    }

    pub fn revoke(&mut self, token: &str) {
        self.expires_at_by_token.remove(token.trim());
    }

    fn prune(&mut self) {
        let now = OffsetDateTime::now_utc();
        self.expires_at_by_token
            .retain(|_, expires_at| *expires_at > now);
    }
}

pub fn platform_token_matches(expected: &str, candidate: &str) -> bool {
    let expected = expected.trim().as_bytes();
    let candidate = candidate.trim().as_bytes();
    expected.ct_eq(candidate).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platform_sessions_are_revocable() {
        let mut sessions = PlatformSessions::default();
        let token = sessions.issue();
        assert_eq!(URL_SAFE_NO_PAD.decode(&token).unwrap().len(), 32);
        assert!(sessions.authenticate(&token));
        sessions.revoke(&token);
        assert!(!sessions.authenticate(&token));
    }

    #[test]
    fn platform_token_comparison_requires_exact_match() {
        assert!(platform_token_matches("admin-secret", "admin-secret"));
        assert!(!platform_token_matches("admin-secret", "admin-other"));
        assert!(!platform_token_matches("admin-secret", "admin-secret-long"));
    }
}
