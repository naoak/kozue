//! kozue command-line interface.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "kozue", about = "A diagram compiler: DSL in, SVG out.")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Render a diagram to SVG.
    Render {
        /// Input `.kzd` file.
        input: PathBuf,
        /// Output SVG file (defaults to `<input>.svg`).
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
    /// Parse and semantically check a diagram, printing `OK` on success.
    Check {
        /// Input `.kzd` file.
        input: PathBuf,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Command::Render { input, output } => run_render(&input, output),
        Command::Check { input } => run_check(&input),
    }
}

fn run_render(input: &Path, output: Option<PathBuf>) -> ExitCode {
    let src = match std::fs::read_to_string(input) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: cannot read {}: {}", input.display(), e);
            return ExitCode::FAILURE;
        }
    };
    let filename = input.to_string_lossy().to_string();

    let diagram = match kozue_dsl::parse(&src) {
        Ok(d) => d,
        Err(errs) => {
            kozue_dsl::report_errors(&filename, &src, &errs);
            return ExitCode::FAILURE;
        }
    };

    let scene = match kozue_layout::layout(&diagram) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: layout failed: {}", e);
            return ExitCode::FAILURE;
        }
    };
    let svg = kozue_render_svg::render(&scene);

    let out_path = output.unwrap_or_else(|| input.with_extension("svg"));
    if let Err(e) = std::fs::write(&out_path, svg) {
        eprintln!("error: cannot write {}: {}", out_path.display(), e);
        return ExitCode::FAILURE;
    }

    ExitCode::SUCCESS
}

fn run_check(input: &Path) -> ExitCode {
    let src = match std::fs::read_to_string(input) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: cannot read {}: {}", input.display(), e);
            return ExitCode::FAILURE;
        }
    };
    let filename = input.to_string_lossy().to_string();

    match kozue_dsl::parse(&src) {
        Ok(_) => {
            println!("OK");
            ExitCode::SUCCESS
        }
        Err(errs) => {
            kozue_dsl::report_errors(&filename, &src, &errs);
            ExitCode::FAILURE
        }
    }
}
