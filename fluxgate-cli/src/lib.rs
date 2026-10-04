//! FluxGate command line interface: profiles, login sessions and API commands.

pub mod api;
pub mod auth;
pub mod cli;
pub mod commands;
pub mod config;
pub mod context;
pub mod error;
pub mod output;
pub mod prompt;

use std::ffi::OsString;
use std::io::Write;

use clap::Parser;

use crate::cli::Cli;
use crate::commands::{App, dispatch};
use crate::config::{ConfigFiles, Env, Overrides, Paths, resolve};
use crate::error::{EXIT_OK, EXIT_USAGE};
use crate::output::{OutputFormat, render};
use crate::prompt::Prompter;

/// Everything `run` takes from the outside world, so tests can replace it.
pub struct Io<'a> {
    pub env: Env,
    /// Whether stdout is a terminal; picks the default output format.
    pub is_tty: bool,
    pub prompter: &'a mut dyn Prompter,
    pub out: &'a mut dyn Write,
    pub err: &'a mut dyn Write,
}

/// Parses `args`, runs the command and returns the process exit code.
pub async fn run<I, T>(args: I, io: Io<'_>) -> i32
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let Io { env, is_tty, prompter, out, err } = io;
    let cli = match Cli::try_parse_from(args) {
        Ok(cli) => cli,
        Err(parse_error) => {
            // --help and --version are "errors" that belong on stdout.
            return if parse_error.use_stderr() {
                let _ = write!(err, "{parse_error}");
                EXIT_USAGE
            } else {
                let _ = write!(out, "{parse_error}");
                EXIT_OK
            };
        }
    };
    let overrides = cli.overrides();
    let paths = match Paths::from_env(&env) {
        Ok(paths) => paths,
        Err(error) => {
            let _ = writeln!(err, "error: {error}");
            return error.exit_code();
        }
    };
    let files = ConfigFiles::load(&paths);
    if let Ok(files) = &files {
        for warning in &files.warnings {
            let _ = writeln!(err, "{warning}");
        }
    }
    let format = files
        .as_ref()
        .ok()
        .and_then(|files| resolve(files, &env, &overrides, is_tty).ok())
        .map(|settings| settings.output.value)
        .unwrap_or_else(|| fallback_format(&overrides, &env, is_tty));

    let mut app = App { env, paths, overrides, is_tty, prompter };
    match dispatch(cli.command, &mut app).await {
        Ok(outcome) => {
            for warning in &outcome.warnings {
                let _ = writeln!(err, "{warning}");
            }
            let text = render(&outcome.value, outcome.kind, format);
            if !text.is_empty() {
                let _ = writeln!(out, "{text}");
            }
            outcome.exit_code
        }
        Err(error) => {
            if format == OutputFormat::Json {
                let body = serde_json::to_string_pretty(&error.to_json()).unwrap_or_default();
                let _ = writeln!(err, "{body}");
            } else {
                let _ = writeln!(err, "error: {error}");
            }
            error.exit_code()
        }
    }
}

/// Output format when the profile cannot be resolved (the error is reported
/// by the command itself).
fn fallback_format(overrides: &Overrides, env: &Env, is_tty: bool) -> OutputFormat {
    overrides
        .output
        .or_else(|| env.get("FLUXGATE_OUTPUT").and_then(|value| value.parse().ok()))
        .unwrap_or(if is_tty { OutputFormat::Table } else { OutputFormat::Json })
}
