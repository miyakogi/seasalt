use std::process::ExitCode;

use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "seasalt",
    version,
    about = "fish-style autosuggestion and per-directory history for bash"
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
        #[arg(trailing_var_arg = true, required = true, allow_hyphen_values = true)]
        cmd: Vec<String>,
    },
    /// Update the exit code of a recorded command (precmd hook)
    Exit {
        #[arg(long)]
        session: String,
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
    /// Search history
    Search {
        #[arg(long, conflicts_with = "all")]
        cwd: Option<String>,
        #[arg(long)]
        all: bool,
        #[arg(long, default_value_t = 100)]
        limit: usize,
        #[arg(long)]
        tsv: bool,
        pattern: String,
    },
    /// Emit shell integration code (bash)
    Init { shell: String },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            // 静かに失敗: シェル統合から呼ばれるため stderr は出さない
            let _ = e;
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<()> {
    let path = seasalt::db::default_db_path()?;
    let conn = seasalt::db::open(&path)?;
    match cli.command {
        Command::Record { cwd, session, cmd } => {
            let cmd = cmd.join(" ");
            let started_at = now_ms();
            let paths = seasalt::paths::required_paths(&cwd, &cmd).join("\0");
            let id = seasalt::db::insert_history(&conn, &cwd, &cmd, started_at, &session, &paths)?;
            println!("{id}");
        }
        Command::Exit {
            session,
            last_id,
            code,
        } => {
            seasalt::db::update_exit_code(&conn, &session, last_id, code)?;
        }
        Command::Suggest { cwd, line } => {
            let line = line.join(" ");
            if let Some(cmd) = seasalt::suggest::suggest(&conn, &cwd, &line)? {
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
            let cwd_filter = if all { None } else { cwd.as_deref() };
            let entries = seasalt::search::search(&conn, cwd_filter, &pattern, limit)?;
            for e in entries {
                if tsv {
                    let code = e.exit_code.map(|c| c.to_string()).unwrap_or_default();
                    println!("{}\t{}\t{}\t{}\t{}", e.id, e.cwd, e.cmd, code, e.started_at);
                } else {
                    println!("{}", e.cmd);
                }
            }
        }
        Command::Init { shell } => match shell.as_str() {
            "bash" => print!("{}", seasalt::integration::bash_init_script()),
            other => anyhow::bail!("unsupported shell: {other}"),
        },
    }
    Ok(())
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
