//! User-Agent string for outgoing HTTP requests.

use std::sync::OnceLock;

const GENERIC_USER_AGENT: &str = "BitGarth (+https://bitgarth.app)";

pub(crate) fn user_agent() -> &'static str {
    static UA: OnceLock<String> = OnceLock::new();
    UA.get_or_init(|| GENERIC_USER_AGENT.to_string())
}

#[cfg(all(test, not(bitgarth_db_unit_only)))]
mod tests {
    use super::*;

    #[test]
    fn user_agent_is_generic_product_identifier() {
        let ua = user_agent();
        assert_eq!(ua, "BitGarth (+https://bitgarth.app)");
        assert!(!ua.contains(env!("CARGO_PKG_VERSION")));
        assert!(!ua.to_ascii_lowercase().contains("docker"));
        assert!(!ua.to_ascii_lowercase().contains(std::env::consts::OS));
    }

    #[test]
    fn user_agent_is_cached() {
        let a: *const str = user_agent();
        let b: *const str = user_agent();
        assert_eq!(a, b);
    }
}
