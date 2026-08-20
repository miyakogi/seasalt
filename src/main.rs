use std::process::ExitCode;
use std::time::Duration;

use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "seasalt",
    version,
    about = "per-directory history and command suggestions for bash and zsh"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Record a command into history (preexec hook)
    Record {
        #[arg(long)]
        cwd: String,
        #[arg(long)]
        session: String,
        #[arg(long, default_value = "bash")]
        shell: String,
        #[arg(trailing_var_arg = true, required = true, allow_hyphen_values = true)]
        cmd: Vec<String>,
    },
    /// Update the exit code of a recorded command (precmd hook)
    Exit {
        #[arg(long)]
        last_id: i64,
        #[arg(long)]
        code: i64,
    },
    /// Suggest the best history match for the current line
    Suggest {
        #[arg(long)]
        cwd: String,
        #[arg(trailing_var_arg = true, required = true, allow_hyphen_values = true)]
        line: Vec<String>,
    },
    /// Search history (scoped to the current directory by default)
    Search {
        #[arg(
            long,
            conflicts_with = "all",
            help = "directory to search (default: the current directory)"
        )]
        cwd: Option<String>,
        #[arg(long)]
        all: bool,
        #[arg(long, default_value_t = 100)]
        limit: usize,
        #[arg(long)]
        tsv: bool,
        pattern: String,
    },
    /// Delete history entries by id (e.g. a command that accidentally recorded a secret)
    Delete {
        #[arg(required = true)]
        ids: Vec<i64>,
    },
    /// Delete all history entries and reclaim the file space (VACUUM)
    Clear,
    /// Emit shell integration code (bash, zsh)
    Init { shell: String },
}

impl Command {
    /// Interactive subcommands (init/search/delete/clear) report errors
    /// on stderr; hook-facing subcommands (record/exit/suggest) fail
    /// silently. Colocated here so new subcommands cannot silently
    /// forget to classify themselves.
    fn interactive(&self) -> bool {
        matches!(
            self,
            Command::Init { .. } | Command::Search { .. } | Command::Delete { .. } | Command::Clear
        )
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let interactive = cli.command.interactive();
    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            if interactive {
                eprintln!("seasalt: {e:#}");
            }
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Command::Record {
            cwd,
            session,
            shell,
            cmd,
        } => {
            let conn = open_db()?;
            let cmd = cmd.join(" ");
            // Do not record commands starting with whitespace,
            // matching HISTCONTROL=ignorespace and the snippet's
            // [[:space:]] guard (spec §4.1). Sensitive input such as
            // passwords is never stored.
            if cmd.chars().next().is_some_and(char::is_whitespace) {
                return Ok(());
            }
            let started_at = now_ms();
            let paths = seasalt::paths::required_paths(&cwd, &cmd).join("\0");
            let id = seasalt::db::record_history(
                &conn, &cwd, &cmd, started_at, &session, &paths, &shell,
            )?;
            if let Some(max) = history_max() {
                seasalt::db::trim_history(&conn, max)?;
            }
            println!("{id}");
        }
        Command::Exit { last_id, code } => {
            let conn = open_db()?;
            seasalt::db::update_exit_code(&conn, last_id, code)?;
        }
        Command::Suggest { cwd, line } => {
            let conn = open_db()?;
            let line = line.join(" ");
            if let Some(cmd) = seasalt::suggest::suggest_budgeted(
                &conn,
                &cwd,
                &line,
                Some(Duration::from_millis(200)),
            )? {
                println!("{cmd}");
            }
        }
        Command::Search {
            cwd,
            all,
            limit,
            tsv,
            pattern,
        } => {
            let conn = open_db()?;
            let cwd_filter = if all {
                None
            } else {
                match cwd {
                    Some(dir) => Some(dir),
                    None => seasalt::search::default_cwd(),
                }
            };
            let entries = seasalt::search::search(&conn, cwd_filter.as_deref(), &pattern, limit)?;
            for e in entries {
                let cmd = escape_cmd(&e.cmd);
                if tsv {
                    let code = e.exit_code.map(|c| c.to_string()).unwrap_or_default();
                    println!(
                        "{}\t{}\t{}\t{}\t{}\t{}",
                        e.id, e.cwd, cmd, code, e.started_at, e.shell
                    );
                } else {
                    println!("{}\t{}", e.id, cmd);
                }
            }
        }
        Command::Delete { ids } => {
            let conn = open_db()?;
            seasalt::db::delete_by_ids(&conn, &ids)?;
        }
        Command::Clear => {
            let conn = open_db()?;
            seasalt::db::clear(&conn)?;
        }
        Command::Init { shell } => match shell.as_str() {
            "bash" => print!("{}", seasalt::integration::bash_init_script()),
            "zsh" => print!("{}", seasalt::integration::zsh_init_script()),
            other => anyhow::bail!("unsupported shell: {other}"),
        },
    }
    Ok(())
}

fn open_db() -> Result<rusqlite::Connection> {
    let path = seasalt::db::default_db_path()?;
    seasalt::db::open(&path)
}

/// Escapes backslash, newline and tab in a command so search output
/// stays one line per entry (multi-line commands are stored raw).
/// suggest output is not escaped: it is the raw command for acceptance.
fn escape_cmd(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('\n', "\\n")
        .replace('\t', "\\t")
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Default history row limit when SEASALT_HISTORY_MAX is unset
const DEFAULT_HISTORY_MAX: usize = 100_000;

/// Resolves the history row limit from SEASALT_HISTORY_MAX. Unset or
/// unparsable values fall back to the default; any value that parses
/// to 0 means unlimited (None). record is hook-facing and silent, so
/// failures are not reported.
fn history_max() -> Option<usize> {
    match std::env::var("SEASALT_HISTORY_MAX") {
        Ok(v) => match v.parse::<usize>() {
            Ok(0) => None,
            Ok(n) => Some(n),
            Err(_) => Some(DEFAULT_HISTORY_MAX),
        },
        Err(_) => Some(DEFAULT_HISTORY_MAX),
    }
}
