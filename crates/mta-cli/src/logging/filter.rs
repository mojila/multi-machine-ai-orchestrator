//! Hand-written diagnostics filter: `target=level` pairs, no `env-filter` (ADR-0011).
//!
//! Grammar (the documented subset):
//!
//! - comma-separated entries, whitespace trimmed, empty entries ignored;
//! - `level` sets the global level;
//! - `target=level` adds a directive; the longest matching target wins, later entries win ties;
//! - target matching is by module-path segment, so `mta_engine` matches `mta_engine::effects`
//!   but not `mta_engine_extra`.
//!
//! An unknown level is an error, not a silently dropped record (docs/observability.md §3).

use std::fmt;

use tracing::level_filters::LevelFilter;

/// A parse failure in a level or directive expression.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilterError(String);

impl FilterError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for FilterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for FilterError {}

/// One `target=level` directive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Directive {
    target: String,
    level: LevelFilter,
}

impl Directive {
    /// The target this directive matches, by path segment.
    #[must_use]
    pub fn target(&self) -> &str {
        &self.target
    }

    /// The level this directive enables.
    #[must_use]
    pub fn level(&self) -> LevelFilter {
        self.level
    }
}

/// The result of parsing one expression: an optional global level plus directives.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParsedExpression {
    /// The last bare level in the expression, if any.
    pub global: Option<LevelFilter>,
    /// The directive list in source order.
    pub directives: Vec<Directive>,
}

/// Parses a single level such as `info`, `DEBUG`, or `off`.
pub fn parse_level(input: &str) -> Result<LevelFilter, FilterError> {
    match input.trim().to_ascii_lowercase().as_str() {
        "error" => Ok(LevelFilter::ERROR),
        "warn" => Ok(LevelFilter::WARN),
        "info" => Ok(LevelFilter::INFO),
        "debug" => Ok(LevelFilter::DEBUG),
        "trace" => Ok(LevelFilter::TRACE),
        "off" => Ok(LevelFilter::OFF),
        other => Err(FilterError::new(format!(
            "unknown level `{other}` (expected error|warn|info|debug|trace|off)"
        ))),
    }
}

/// Parses a list of directive strings (each may also contain commas).
pub fn parse_directives(entries: &[String]) -> Result<Vec<Directive>, FilterError> {
    let joined = entries.join(",");
    Ok(parse_expression(&joined)?.directives)
}

/// Parses a `RUST_LOG`-style expression.
pub fn parse_expression(input: &str) -> Result<ParsedExpression, FilterError> {
    let mut parsed = ParsedExpression::default();
    for raw in input.split(',') {
        let entry = raw.trim();
        if entry.is_empty() {
            continue;
        }
        match entry.split_once('=') {
            Some((target, level)) => {
                let target = target.trim();
                if target.is_empty() {
                    return Err(FilterError::new(format!(
                        "directive `{entry}` has an empty target"
                    )));
                }
                let level = parse_level(level)?;
                parsed.directives.push(Directive {
                    target: target.to_owned(),
                    level,
                });
            }
            None => parsed.global = Some(parse_level(entry)?),
        }
    }
    Ok(parsed)
}

fn target_matches(directive: &str, target: &str) -> bool {
    target == directive
        || target
            .strip_prefix(directive)
            .is_some_and(|rest| rest.starts_with("::"))
}

/// A resolved filter: one global level and a directive table.
#[derive(Debug, Clone)]
pub struct FilterSpec {
    global: LevelFilter,
    directives: Vec<Directive>,
}

impl FilterSpec {
    /// Builds a filter from an already-parsed global level and directives.
    #[must_use]
    pub fn new(global: LevelFilter, directives: Vec<Directive>) -> Self {
        Self { global, directives }
    }

    /// The global level, used when no directive matches.
    #[must_use]
    pub fn global(&self) -> LevelFilter {
        self.global
    }

    /// Returns whether a record at `target` and `level` is enabled.
    ///
    /// The longest matching target wins; later entries win ties. A directive may lower the
    /// level (to `off`) as well as raise it, which is what makes `hyper=warn` useful at `debug`.
    #[must_use]
    pub fn enabled(&self, target: &str, level: LevelFilter) -> bool {
        let winner = self
            .directives
            .iter()
            .enumerate()
            .filter(|(_, directive)| target_matches(directive.target(), target))
            .max_by_key(|(index, directive)| (directive.target().len(), *index))
            .map(|(_, directive)| directive.level());

        let effective = winner.unwrap_or(self.global);
        level <= effective
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(expression: &str) -> FilterSpec {
        let parsed = parse_expression(expression).expect("expression parses");
        FilterSpec::new(
            parsed.global.unwrap_or(LevelFilter::INFO),
            parsed.directives,
        )
    }

    #[test]
    fn levels_are_case_insensitive_and_complete() {
        for (text, expected) in [
            ("error", LevelFilter::ERROR),
            ("WaRn", LevelFilter::WARN),
            ("info", LevelFilter::INFO),
            ("debug", LevelFilter::DEBUG),
            ("trace", LevelFilter::TRACE),
            ("off", LevelFilter::OFF),
        ] {
            assert_eq!(parse_level(text).expect("level"), expected, "{text}");
        }
        assert!(parse_level("verbose").is_err());
    }

    #[test]
    fn longest_target_wins() {
        let filter = spec("warn,mta_cli=info,mta_cli::logging=debug");
        assert!(filter.enabled("mta_cli::logging::sink", LevelFilter::DEBUG));
        assert!(!filter.enabled("mta_cli::logging::sink", LevelFilter::TRACE));
        assert!(filter.enabled("mta_cli::config", LevelFilter::INFO));
        assert!(!filter.enabled("mta_cli::config", LevelFilter::DEBUG));
        assert!(filter.enabled("hyper::client", LevelFilter::WARN));
        assert!(!filter.enabled("hyper::client", LevelFilter::INFO));
    }

    #[test]
    fn target_matching_respects_path_boundaries() {
        let filter = spec("info,mta_cli=debug");
        assert!(filter.enabled("mta_cli", LevelFilter::DEBUG));
        assert!(filter.enabled("mta_cli::main", LevelFilter::DEBUG));
        assert!(!filter.enabled("mta_cli_extra", LevelFilter::DEBUG));
    }

    #[test]
    fn later_directive_wins_a_tie() {
        let filter = spec("info,mta_cli=debug,mta_cli=off");
        assert!(!filter.enabled("mta_cli::main", LevelFilter::ERROR));
    }

    #[test]
    fn off_disables_everything() {
        let filter = spec("off");
        assert!(!filter.enabled("anything", LevelFilter::ERROR));
    }

    #[test]
    fn bare_level_is_the_default_and_last_one_wins() {
        let parsed = parse_expression("warn,info").expect("parses");
        assert_eq!(parsed.global, Some(LevelFilter::INFO));
        assert!(parsed.directives.is_empty());
    }

    #[test]
    fn malformed_entries_are_errors() {
        assert!(parse_expression("mta=").is_err());
        assert!(parse_expression("=debug").is_err());
        assert!(parse_expression("nope=loud").is_err());
    }

    #[test]
    fn directive_lists_may_contain_commas() {
        let directives =
            parse_directives(&["mta_cli=debug,hyper=warn".to_owned()]).expect("parses");
        assert_eq!(directives.len(), 2);
    }
}
