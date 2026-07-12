use clap::Parser;
use foundry_config::TronConfig;

/// CLI options for Tron transactions.
///
/// These flags override the corresponding fields of the `[tron]` config section. Only the two most
/// commonly tuned knobs are exposed; the remaining protobuf parameters (`origin_energy_limit`,
/// `user_fee_percentage`) are configured via `foundry.toml`.
#[derive(Clone, Copy, Debug, Default, Parser)]
#[command(next_help_heading = "Tron")]
pub struct TronOpts {
    /// Maximum TRX, in SUN, that may be burned for a Tron transaction (`fee_limit`).
    ///
    /// Overrides `[tron] fee_limit`. 1 TRX = 1_000_000 SUN.
    #[arg(long = "tron.fee-limit", id = "tron_fee_limit", value_name = "SUN")]
    pub fee_limit: Option<i64>,

    /// Transaction expiration window, in seconds, for Tron transactions.
    ///
    /// Overrides `[tron] expiration`.
    #[arg(long = "tron.expiration", id = "tron_expiration", value_name = "SECONDS")]
    pub expiration: Option<u64>,
}

impl TronOpts {
    /// Returns `true` if any Tron-specific option is set.
    pub const fn is_tron(&self) -> bool {
        self.fee_limit.is_some() || self.expiration.is_some()
    }

    /// Applies the CLI overrides on top of the given [`TronConfig`], returning the merged config.
    ///
    /// CLI flags take precedence over the `foundry.toml` values.
    pub const fn apply(&self, cfg: &TronConfig) -> TronConfig {
        let mut out = *cfg;
        if let Some(fee_limit) = self.fee_limit {
            out.fee_limit = fee_limit;
        }
        if let Some(expiration) = self.expiration {
            out.expiration = expiration;
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_tron_flags() {
        let opts =
            TronOpts::try_parse_from(["", "--tron.fee-limit", "5", "--tron.expiration", "30"])
                .unwrap();
        assert_eq!(opts.fee_limit, Some(5));
        assert_eq!(opts.expiration, Some(30));
        assert!(opts.is_tron());
    }

    #[test]
    fn defaults_are_unset() {
        let opts = TronOpts::try_parse_from([""]).unwrap();
        assert!(opts.fee_limit.is_none());
        assert!(opts.expiration.is_none());
        assert!(!opts.is_tron());
    }

    #[test]
    fn apply_overrides_config() {
        let cfg = TronConfig::default();
        let opts = TronOpts { fee_limit: Some(400_000_000), expiration: None };
        let merged = opts.apply(&cfg);
        // Overridden field takes the CLI value.
        assert_eq!(merged.fee_limit, 400_000_000);
        // Untouched fields keep the config defaults.
        assert_eq!(merged.expiration, cfg.expiration);
        assert_eq!(merged.origin_energy_limit, cfg.origin_energy_limit);
        assert_eq!(merged.user_fee_percentage, cfg.user_fee_percentage);
    }
}
