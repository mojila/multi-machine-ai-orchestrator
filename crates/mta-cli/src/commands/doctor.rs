//! `mta doctor` — health and diagnostics reporting. M0 checks config, paths, and logs; provider,
//! event, host, and estimation checks arrive with the milestones that create those systems.

use std::fs;
use std::io::Write;
use std::path::Path;

use crate::cli::DoctorArgs;
use crate::config::LoadedConfig;
use crate::error::AppError;
use crate::logging::{self, resolve::ResolvedLogging};
use crate::paths::Paths;

/// Runs the checks.
pub fn run(
    paths: &Paths,
    loaded: &LoadedConfig,
    resolved: &ResolvedLogging,
    logging: &crate::logging::LoggingGuard,
    args: &DoctorArgs,
) -> Result<(), AppError> {
    let mut out = std::io::stdout().lock();

    emit(&mut out, format_args!("mta {}", env!("CARGO_PKG_VERSION")))?;
    emit(
        &mut out,
        format_args!(
            "config          {} ({})",
            paths.config_file.display(),
            if loaded.exists {
                "present"
            } else {
                "absent; defaults"
            }
        ),
    )?;
    if let Some(hash) = &loaded.hash {
        emit(&mut out, format_args!("config.sha256   {hash}"))?;
    }
    emit(
        &mut out,
        format_args!("database        {} (M1)", paths.db_file.display()),
    )?;
    emit(
        &mut out,
        format_args!("state           {}", paths.state_dir.display()),
    )?;
    emit(
        &mut out,
        format_args!("logs            {}", resolved.dir.display()),
    )?;

    if args.logs {
        emit(&mut out, format_args!(""))?;
        emit(&mut out, format_args!("logging:"))?;
        emit(
            &mut out,
            format_args!(
                "  level         {} ({})",
                resolved.filter.global(),
                resolved.level_source.describe(&resolved.config_path)
            ),
        )?;
        emit(
            &mut out,
            format_args!(
                "  directives    {} ({})",
                if resolved.directives.is_empty() {
                    "(none)".to_owned()
                } else {
                    resolved.directives.join(", ")
                },
                resolved.directives_source.describe(&resolved.config_path)
            ),
        )?;
        emit(
            &mut out,
            format_args!(
                "  format        {} ({})",
                resolved.format,
                resolved.format_source.describe(&resolved.config_path)
            ),
        )?;
        emit(
            &mut out,
            format_args!(
                "  ansi          {} (resolved: {})",
                resolved.ansi_mode,
                if resolved.ansi { "on" } else { "off" }
            ),
        )?;
        emit(
            &mut out,
            format_args!(
                "  console       {}{}",
                if resolved.console {
                    "on (stderr)"
                } else {
                    "off"
                },
                if resolved.console_forced_off {
                    " (disabled by TUI mode)"
                } else {
                    ""
                }
            ),
        )?;
        emit(
            &mut out,
            format_args!(
                "  file          {}{}",
                if resolved.file { "on" } else { "off" },
                if resolved.file_forced_on {
                    " (forced on by TUI mode)"
                } else {
                    ""
                }
            ),
        )?;
        emit(
            &mut out,
            format_args!("  directory     {}", resolved.dir.display()),
        )?;
        emit(
            &mut out,
            format_args!(
                "  rotation      {} (retain {} files)",
                resolved.rotation, resolved.retain_files
            ),
        )?;
        emit(
            &mut out,
            format_args!(
                "  size caps     {} MiB/file, {} MiB total",
                resolved.max_file_mb, resolved.max_total_mb
            ),
        )?;
        emit(
            &mut out,
            format_args!(
                "  content opts  prompts={} tool_output={} max_field_bytes={}",
                resolved.log_prompts, resolved.log_tool_output, resolved.max_field_bytes
            ),
        )?;
        emit(
            &mut out,
            format_args!(
                "  ring / dedup  {} lines / {} ms (TUI overlay, M2)",
                resolved.ring_lines, resolved.dedup_window_ms
            ),
        )?;

        emit(&mut out, format_args!(""))?;
        // The writer thread buffers, so an unflushed run would report an empty "recent records"
        // section while holding its own records in memory. Flush before looking at the files.
        logging.flush();
        report_log_files(&mut out, &resolved.dir)?;
    }

    Ok(())
}

fn emit(out: &mut impl Write, args: std::fmt::Arguments<'_>) -> Result<(), AppError> {
    writeln!(out, "{args}").map_err(|error| AppError::io("writing to stdout", error))
}

fn report_log_files(out: &mut impl Write, dir: &Path) -> Result<(), AppError> {
    let files = logging::log_files(dir)
        .map_err(|error| AppError::io(format!("reading {}", dir.display()), error))?;
    let total: u64 = files.iter().map(|(_, size, _)| *size).sum();
    emit(
        out,
        format_args!("log files: {} ({} total)", files.len(), human_bytes(total)),
    )?;
    for (path, size, _) in files.iter().rev().take(10) {
        let name = path.file_name().map_or_else(
            || path.display().to_string(),
            |name| name.to_string_lossy().into_owned(),
        );
        emit(out, format_args!("  {name:<40} {}", human_bytes(*size)))?;
    }

    let crashes = fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .filter(|entry| {
                    entry
                        .file_name()
                        .to_str()
                        .is_some_and(|name| name.starts_with("crash-"))
                })
                .count()
        })
        .unwrap_or(0);
    emit(
        out,
        format_args!("crash markers: {crashes} (reported and removed on the next startup)"),
    )?;

    if let Some((newest, _, _)) = files.last() {
        emit(out, format_args!("recent records ({}):", newest.display()))?;
        let text = fs::read_to_string(newest)
            .map_err(|error| AppError::io(format!("reading {}", newest.display()), error))?;
        let lines = text.lines().rev().take(10).collect::<Vec<_>>();
        for line in lines.into_iter().rev() {
            emit(out, format_args!("  {line}"))?;
        }
    } else {
        emit(out, format_args!("recent records: (none yet)"))?;
    }
    Ok(())
}

fn human_bytes(bytes: u64) -> String {
    const KIB: u64 = 1024;
    const MIB: u64 = 1024 * KIB;
    if bytes >= MIB {
        format!("{:.1} MiB", bytes as f64 / MIB as f64)
    } else if bytes >= KIB {
        format!("{:.1} KiB", bytes as f64 / KIB as f64)
    } else {
        format!("{bytes} B")
    }
}
