use std::io::{IsTerminal, Write};

use fluxgate_cli::config::Env;
use fluxgate_cli::prompt::TerminalPrompter;
use fluxgate_cli::{Io, run};

#[tokio::main]
async fn main() {
    let mut prompter = TerminalPrompter;
    let mut out = std::io::stdout();
    let mut err = std::io::stderr();
    let is_tty = out.is_terminal();
    let code = run(
        std::env::args_os(),
        Io { env: Env::from_process(), is_tty, prompter: &mut prompter, out: &mut out, err: &mut err },
    )
    .await;
    let _ = out.flush();
    std::process::exit(code);
}
