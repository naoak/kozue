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
    Plantuml,
}

/// Output format selector.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Default)]
enum Format {
    #[default]
    Svg,
    Term,
    Png,
    Drawio,
    Dot,
    Excalidraw,
    Pptx,
}

#[derive(Subcommand)]
enum Command {
    /// Render a diagram to SVG or terminal text.
    Render {
        /// Input `.kzd` / `.mmd` / `.mermaid` file.
        input: PathBuf,
        /// Output file (defaults to `<input>.svg` for svg, stdout for term).
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// Override the frontend language (auto-detected from extension by default).
        #[arg(long)]
        lang: Option<Lang>,
        /// Output format: `svg` (default), `term` (plain-text terminal), `png` (raster PNG), `drawio` (mxGraph XML), `dot` (Graphviz DOT), `excalidraw` (Excalidraw JSON), or `pptx` (PowerPoint shapes).
        #[arg(long, default_value = "svg")]
        format: Format,
    },
    /// Parse and semantically check a diagram, printing `OK` on success.
    Check {
        /// Input file.
        input: PathBuf,
        /// Override the frontend language.
        #[arg(long)]
        lang: Option<Lang>,
    },
    /// Format a kozue DSL (.kzd) file into canonical normal form.
    ///
    /// By default the file is rewritten in-place (only if changed).
    /// Use `--check` for CI (exits non-zero if the file would change) or
    /// `--stdout` to write the result to stdout instead of the file.
    Fmt {
        /// Input `.kzd` file.
        input: PathBuf,
        /// Exit non-zero if the file is not already in canonical form (no rewrite).
        #[arg(long)]
        check: bool,
        /// Write formatted output to stdout instead of rewriting the file.
        #[arg(long)]
        stdout: bool,
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
    Plantuml,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Command::Render {
            input,
            output,
            lang,
            format,
        } => run_render(&input, output, lang, format),
        Command::Check { input, lang } => run_check(&input, lang),
        Command::Fmt {
            input,
            check,
            stdout,
        } => run_fmt(&input, check, stdout),
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
        Some("puml") | Some("plantuml") | Some("pu") | Some("iuml") => Lang::Plantuml,
        _ => Lang::Kozue,
    }
}

fn run_render(
    input: &Path,
    output: Option<PathBuf>,
    lang: Option<Lang>,
    format: Format,
) -> ExitCode {
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
        Lang::Plantuml => match kozue_plantuml::parse(&src) {
            Ok(d) => d,
            Err(errs) => {
                kozue_plantuml::report_errors(&filename, &src, &errs);
                return ExitCode::FAILURE;
            }
        },
    };

    let layout_out = match kozue_layout::layout_full(&diagram) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("error: layout failed: {}", e);
            return ExitCode::FAILURE;
        }
    };
    let export_input = if matches!(format, Format::Drawio | Format::Excalidraw | Format::Pptx) {
        match layout_out.export_input(&diagram) {
            Ok(input) => Some(input),
            Err(error) => {
                eprintln!("error: export contract failed: {error}");
                return ExitCode::FAILURE;
            }
        }
    } else {
        None
    };

    match format {
        Format::Svg => {
            let svg = kozue_render_svg::render(&layout_out.scene);
            let out_path = output.unwrap_or_else(|| input.with_extension("svg"));
            if let Err(e) = std::fs::write(&out_path, svg) {
                eprintln!("error: cannot write {}: {}", out_path.display(), e);
                return ExitCode::FAILURE;
            }
        }
        Format::Term => {
            let text = kozue_render_term::render(&layout_out.scene);
            match output {
                Some(out_path) => {
                    if let Err(e) = std::fs::write(&out_path, text) {
                        eprintln!("error: cannot write {}: {}", out_path.display(), e);
                        return ExitCode::FAILURE;
                    }
                }
                None => {
                    print!("{text}");
                }
            }
        }
        Format::Png => {
            let png = match kozue_render_png::render(&layout_out.scene) {
                Ok(bytes) => bytes,
                Err(e) => {
                    eprintln!("error: PNG render failed: {}", e);
                    return ExitCode::FAILURE;
                }
            };
            let out_path = output.unwrap_or_else(|| input.with_extension("png"));
            if let Err(e) = std::fs::write(&out_path, &png) {
                eprintln!("error: cannot write {}: {}", out_path.display(), e);
                return ExitCode::FAILURE;
            }
        }
        Format::Drawio => {
            let drawio = match kozue_render_drawio::render_export(export_input.as_ref().unwrap()) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("error: draw.io export failed: {}", e);
                    return ExitCode::FAILURE;
                }
            };
            let out_path = output.unwrap_or_else(|| input.with_extension("drawio"));
            if let Err(e) = std::fs::write(&out_path, &drawio) {
                eprintln!("error: cannot write {}: {}", out_path.display(), e);
                return ExitCode::FAILURE;
            }
        }
        Format::Dot => {
            // DOT is a graph *description*; Graphviz lays it out itself, so the
            // exporter reads the semantic diagram directly and ignores `scene`.
            let dot = match kozue_render_dot::render(&diagram) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("error: DOT export failed: {}", e);
                    return ExitCode::FAILURE;
                }
            };
            let out_path = output.unwrap_or_else(|| input.with_extension("dot"));
            if let Err(e) = std::fs::write(&out_path, &dot) {
                eprintln!("error: cannot write {}: {}", out_path.display(), e);
                return ExitCode::FAILURE;
            }
        }
        Format::Excalidraw => {
            let excalidraw =
                match kozue_render_excalidraw::render_export(export_input.as_ref().unwrap()) {
                    Ok(s) => s,
                    Err(e) => {
                        eprintln!("error: Excalidraw export failed: {}", e);
                        return ExitCode::FAILURE;
                    }
                };
            let out_path = output.unwrap_or_else(|| input.with_extension("excalidraw"));
            if let Err(e) = std::fs::write(&out_path, &excalidraw) {
                eprintln!("error: cannot write {}: {}", out_path.display(), e);
                return ExitCode::FAILURE;
            }
        }
        Format::Pptx => {
            let pptx = match kozue_render_pptx::render_export(export_input.as_ref().unwrap()) {
                Ok(bytes) => bytes,
                Err(e) => {
                    eprintln!("error: PowerPoint export failed: {}", e);
                    return ExitCode::FAILURE;
                }
            };
            let out_path = output.unwrap_or_else(|| input.with_extension("pptx"));
            if let Err(e) = std::fs::write(&out_path, &pptx) {
                eprintln!("error: cannot write {}: {}", out_path.display(), e);
                return ExitCode::FAILURE;
            }
        }
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
        Lang::Plantuml => kozue_plantuml::parse(&src).map(|_| ()).map_err(|errs| {
            kozue_plantuml::report_errors(&filename, &src, &errs);
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

fn run_fmt(input: &Path, check: bool, stdout: bool) -> ExitCode {
    // Reject Mermaid and PlantUML files immediately.
    match input
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .as_deref()
    {
        Some("mmd") | Some("mermaid") => {
            eprintln!("error: fmt is not supported for Mermaid input");
            return ExitCode::FAILURE;
        }
        Some("puml") | Some("plantuml") | Some("pu") | Some("iuml") => {
            eprintln!("error: fmt is not supported for PlantUML input");
            return ExitCode::FAILURE;
        }
        _ => {}
    }

    let src = match std::fs::read_to_string(input) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: cannot read {}: {}", input.display(), e);
            return ExitCode::FAILURE;
        }
    };
    let filename = input.to_string_lossy().to_string();

    let formatted = match kozue_dsl::format_kzd(&src) {
        Ok(s) => s,
        Err(errs) => {
            kozue_dsl::report_errors(&filename, &src, &errs);
            return ExitCode::FAILURE;
        }
    };

    if stdout {
        print!("{}", formatted);
        return ExitCode::SUCCESS;
    }

    if check {
        if formatted == src {
            ExitCode::SUCCESS
        } else {
            eprintln!("error: {} is not formatted", input.display());
            ExitCode::FAILURE
        }
    } else {
        // In-place rewrite (only if changed).
        if formatted != src {
            if let Err(e) = std::fs::write(input, &formatted) {
                eprintln!("error: cannot write {}: {}", input.display(), e);
                return ExitCode::FAILURE;
            }
        }
        ExitCode::SUCCESS
    }
}

fn run_compat(language: CompatLang) -> ExitCode {
    match language {
        CompatLang::Mermaid => print_mermaid_compat(),
        CompatLang::Plantuml => print_plantuml_compat(),
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

fn print_plantuml_compat() {
    use kozue_plantuml::features::{Support, FEATURES};

    println!("PlantUML compatibility — kozue-plantuml frontend");
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
