//! kozue command-line interface.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand, ValueEnum};

#[derive(Parser)]
#[command(name = "kozue", about = "A diagram compiler: DSL in, SVG out.")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

/// Language selector for explicit frontend override.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum Lang {
    Kozue,
    Mermaid,
}

#[derive(Subcommand)]
enum Command {
    /// Render a diagram to SVG.
    Render {
        /// Input `.kzd` / `.mmd` / `.mermaid` file.
        input: PathBuf,
        /// Output SVG file (defaults to `<input>.svg`).
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// Override the frontend language (auto-detected from extension by default).
        #[arg(long)]
        lang: Option<Lang>,
    },
    /// Parse and semantically check a diagram, printing `OK` on success.
    Check {
        /// Input file.
        input: PathBuf,
        /// Override the frontend language.
        #[arg(long)]
        lang: Option<Lang>,
    },
    /// Display compatibility information for a supported language frontend.
    Compat {
        /// Language to show compatibility for.
        language: CompatLang,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum CompatLang {
    Mermaid,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Command::Render {
            input,
            output,
            lang,
        } => run_render(&input, output, lang),
        Command::Check { input, lang } => run_check(&input, lang),
        Command::Compat { language } => run_compat(language),
    }
}

/// Detect which language frontend to use based on file extension and optional override.
fn detect_lang(input: &Path, lang: Option<Lang>) -> Lang {
    if let Some(l) = lang {
        return l;
    }
    match input
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .as_deref()
    {
        Some("mmd") | Some("mermaid") => Lang::Mermaid,
        _ => Lang::Kozue,
    }
}

fn run_render(input: &Path, output: Option<PathBuf>, lang: Option<Lang>) -> ExitCode {
    let src = match std::fs::read_to_string(input) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: cannot read {}: {}", input.display(), e);
            return ExitCode::FAILURE;
        }
    };
    let filename = input.to_string_lossy().to_string();

    let diagram = match detect_lang(input, lang) {
        Lang::Kozue => match kozue_dsl::parse(&src) {
            Ok(d) => d,
            Err(errs) => {
                kozue_dsl::report_errors(&filename, &src, &errs);
                return ExitCode::FAILURE;
            }
        },
        Lang::Mermaid => match kozue_mermaid::parse(&src) {
            Ok(d) => d,
            Err(errs) => {
                kozue_mermaid::report_errors(&filename, &src, &errs);
                return ExitCode::FAILURE;
            }
        },
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

fn run_check(input: &Path, lang: Option<Lang>) -> ExitCode {
    let src = match std::fs::read_to_string(input) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: cannot read {}: {}", input.display(), e);
            return ExitCode::FAILURE;
        }
    };
    let filename = input.to_string_lossy().to_string();

    let result = match detect_lang(input, lang) {
        Lang::Kozue => kozue_dsl::parse(&src).map(|_| ()).map_err(|errs| {
            kozue_dsl::report_errors(&filename, &src, &errs);
        }),
        Lang::Mermaid => kozue_mermaid::parse(&src).map(|_| ()).map_err(|errs| {
            kozue_mermaid::report_errors(&filename, &src, &errs);
        }),
    };

    match result {
        Ok(()) => {
            println!("OK");
            ExitCode::SUCCESS
        }
        Err(()) => ExitCode::FAILURE,
    }
}

fn run_compat(language: CompatLang) -> ExitCode {
    match language {
        CompatLang::Mermaid => print_mermaid_compat(),
    }
    ExitCode::SUCCESS
}

fn print_mermaid_compat() {
    use kozue_mermaid::features::{Support, FEATURES};

    println!("Mermaid compatibility — kozue-mermaid frontend");
    println!();
    // Column widths.
    let name_w = FEATURES.iter().map(|f| f.name.len()).max().unwrap_or(10) + 2;
    let status_w = 11usize;
    println!(
        "{:<name_w$} {:<status_w$} Notes",
        "Feature",
        "Status",
        name_w = name_w,
        status_w = status_w,
    );
    println!("{}", "-".repeat(name_w + status_w + 40));
    for f in FEATURES {
        let status = format!("{} {}", f.support.symbol(), f.support.as_str());
        println!(
            "{:<name_w$} {:<status_w$} {}",
            f.name,
            status,
            f.note,
            name_w = name_w,
            status_w = status_w,
        );
    }
    println!();
    let supported = FEATURES
        .iter()
        .filter(|f| f.support == Support::Supported)
        .count();
    let partial = FEATURES
        .iter()
        .filter(|f| f.support == Support::Partial)
        .count();
    let unsupported = FEATURES
        .iter()
        .filter(|f| f.support == Support::Unsupported)
        .count();
    println!(
        "Total: {} supported, {} partial, {} unsupported (out of {})",
        supported,
        partial,
        unsupported,
        FEATURES.len(),
    );
}
