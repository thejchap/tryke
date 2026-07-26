use std::env;

use anyhow::{Result, anyhow};
use log::LevelFilter;
use tryke_reporter::Verbosity;

const EXPECTED_LEVELS: &str = "off, error, warn, info, debug, or trace";

/// Process-wide logging configuration resolved from the CLI and environment.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct LogConfig {
    level: LevelFilter,
}

impl LogConfig {
    pub(crate) fn from_env(cli_level: LevelFilter) -> Result<Self> {
        match env::var("TRYKE_LOG") {
            Ok(value) => Self::resolve(Some(&value), cli_level),
            Err(env::VarError::NotPresent) => Self::resolve(None, cli_level),
            Err(env::VarError::NotUnicode(_)) => Err(anyhow!(
                "TRYKE_LOG must be valid Unicode; expected {EXPECTED_LEVELS}"
            )),
        }
    }

    fn resolve(tryke_log: Option<&str>, cli_level: LevelFilter) -> Result<Self> {
        let Some(value) = tryke_log else {
            return Ok(Self { level: cli_level });
        };
        let level = value.trim().parse::<LevelFilter>().map_err(|_| {
            anyhow!("invalid TRYKE_LOG value {value:?}; expected {EXPECTED_LEVELS}")
        })?;
        Ok(Self { level })
    }

    pub(crate) fn init_rust_logging(self) {
        env_logger::Builder::from_env(
            env_logger::Env::default().default_filter_or(self.level.as_str().to_ascii_lowercase()),
        )
        .init();
    }

    pub(crate) fn level(self) -> LevelFilter {
        self.level
    }

    pub(crate) fn reporter_verbosity(self) -> Verbosity {
        Verbosity::from_level_filter(self.level)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_level_is_used_without_tryke_log() {
        for level in [
            LevelFilter::Off,
            LevelFilter::Error,
            LevelFilter::Warn,
            LevelFilter::Info,
            LevelFilter::Debug,
            LevelFilter::Trace,
        ] {
            let config = LogConfig::resolve(None, level).expect("resolve log configuration");
            assert_eq!(config.level(), level);
        }
    }

    #[test]
    fn tryke_log_overrides_cli_level() {
        for (value, level) in [
            ("off", LevelFilter::Off),
            ("error", LevelFilter::Error),
            ("warn", LevelFilter::Warn),
            ("info", LevelFilter::Info),
            ("debug", LevelFilter::Debug),
            ("trace", LevelFilter::Trace),
        ] {
            let config =
                LogConfig::resolve(Some(value), LevelFilter::Off).expect("resolve TRYKE_LOG");
            assert_eq!(config.level(), level);
        }
    }

    #[test]
    fn tryke_log_is_trimmed_and_case_insensitive() {
        let config =
            LogConfig::resolve(Some("  DeBuG  "), LevelFilter::Warn).expect("resolve TRYKE_LOG");
        assert_eq!(config.level(), LevelFilter::Debug);
    }

    #[test]
    fn invalid_tryke_log_is_an_error() {
        let error = LogConfig::resolve(Some("verbose"), LevelFilter::Warn)
            .expect_err("invalid TRYKE_LOG should fail");
        assert_eq!(
            error.to_string(),
            "invalid TRYKE_LOG value \"verbose\"; expected off, error, warn, info, debug, or trace"
        );
    }

    #[test]
    fn reporter_verbosity_uses_resolved_level() {
        assert!(matches!(
            LogConfig {
                level: LevelFilter::Off
            }
            .reporter_verbosity(),
            Verbosity::Quiet
        ));
        assert!(matches!(
            LogConfig {
                level: LevelFilter::Error
            }
            .reporter_verbosity(),
            Verbosity::Quiet
        ));
        assert!(matches!(
            LogConfig {
                level: LevelFilter::Warn
            }
            .reporter_verbosity(),
            Verbosity::Normal
        ));
        for level in [LevelFilter::Info, LevelFilter::Debug, LevelFilter::Trace] {
            assert!(matches!(
                LogConfig { level }.reporter_verbosity(),
                Verbosity::Verbose
            ));
        }
    }
}
