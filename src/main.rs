use std::env;
use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use inquire::Select;
use log::{error, Level, LevelFilter};
use spritepack::{
    compute_nesting_keys, discover_sprite_set_folders, find_asphalt_toml, find_spritepack_toml,
    folder_label_relative, load_config, module_local_name_from_require_path,
    normalize_require_path, pack_folder, resolve_against_config, PackOptions, RequireInfo,
    MAX_SHEET_HEIGHT, MAX_SHEET_WIDTH,
};
use std::io::{self, BufRead, IsTerminal, Write};

use console::style;

#[derive(Parser)]
#[command(name = "spritepack", version, about)]
struct Cli {
    /// Path to spritepack.toml (default: search upward from the current directory)
    #[arg(long, global = true)]
    config: Option<PathBuf>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Interactive run: pick a sprites folder, then pack sheets + Luau.
    Run {
        /// Skip prompts: folder relative to `paths.path` (e.g. `Brainrots` or `Pack/Mobs`)
        #[arg(long)]
        folder: Option<String>,

        /// Forces atlas cell width. Must be used with `--cell-height` (or omit both for auto).
        #[arg(long)]
        cell_width: Option<u32>,

        /// Forces atlas cell height. Must be used with `--cell-width` (or omit both for auto).
        #[arg(long)]
        cell_height: Option<u32>,
    },
}

fn init_logging() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .filter_level(LevelFilter::Info)
        .format(move |buf, record| {
            let tag = match record.level() {
                Level::Error => style("error").red(),
                Level::Warn => style("warn").yellow(),
                Level::Info => style("info").green(),
                Level::Debug => style("debug").cyan(),
                Level::Trace => style("trace").magenta(),
            }
            .bold();

            writeln!(buf, "{}{} {}", tag, style(":").bold(), record.args())
        })
        .init();
}

fn validate_cell_pair(label: &str, w: Option<u32>, h: Option<u32>) -> Result<Option<(u32, u32)>> {
    match (w, h) {
        (None, None) => Ok(None),
        (Some(w), Some(h)) => Ok(Some((w, h))),
        _ => bail!(
            "{label}: set both cell width and height, or neither (neither = prompt in a terminal, or use source size when stdin is not a TTY)"
        ),
    }
}

fn parse_u32_field(raw: &str, label: &str) -> Result<u32> {
    let t = raw.trim();
    t.parse::<u32>()
        .with_context(|| format!("invalid {label}: {t:?}"))
}

fn read_line_trimmed() -> Result<String> {
    let mut buf = String::new();
    io::stdin()
        .lock()
        .read_line(&mut buf)
        .context("read stdin")?;
    Ok(buf.trim().to_string())
}

fn prompt_cell_size(max_sheet_w: u32, max_sheet_h: u32) -> Result<(u32, u32)> {
    let mut out = io::stdout().lock();
    write!(out, "Cell width ({} max): ", max_sheet_w)?;
    out.flush().context("flush stdout")?;
    drop(out);
    let w_raw = read_line_trimmed()?;
    let w = if w_raw.is_empty() {
        max_sheet_w
    } else {
        parse_u32_field(&w_raw, "cell width")?
    };
    if w > max_sheet_w {
        bail!(
            "cell width {w} is wider than atlas limit {max_sheet_w}px (built-in); enter a smaller cell"
        );
    }

    let mut out = io::stdout().lock();
    write!(out, "Cell height ({} max): ", max_sheet_h)?;
    out.flush().context("flush stdout")?;
    drop(out);
    let h_raw = read_line_trimmed()?;
    let h = if h_raw.is_empty() {
        max_sheet_h
    } else {
        parse_u32_field(&h_raw, "cell height")?
    };
    if h > max_sheet_h {
        bail!(
            "cell height {h} is taller than atlas limit {max_sheet_h}px (built-in); enter a smaller cell"
        );
    }
    if w == 0 || h == 0 {
        bail!("cell width and height must be greater than zero");
    }
    Ok((w, h))
}

/// Build `RequireInfo` from the optional `require_path` in config.
fn resolve_require(
    raw: Option<&str>,
    nesting_keys: Vec<String>,
) -> Result<Option<RequireInfo>> {
    let raw = match raw {
        Some(s) if !s.trim().is_empty() => s,
        _ => return Ok(None),
    };
    let require_path = normalize_require_path(raw);
    if require_path == "@" {
        return Ok(None);
    }
    let local_name = module_local_name_from_require_path(&require_path)?;
    Ok(Some(RequireInfo {
        require_path,
        local_name,
        nesting_keys,
    }))
}

fn run_command(
    config_path: Option<PathBuf>,
    folder: Option<String>,
    cell_width: Option<u32>,
    cell_height: Option<u32>,
) -> Result<()> {
    let cwd = env::current_dir().context("current_dir")?;
    let cfg_path = config_path.or_else(|| find_spritepack_toml(&cwd)).context(
        "spritepack.toml not found (use --config or run from a directory under your project)",
    )?;

    let cfg = load_config(&cfg_path)?;

    let sprites_root = resolve_against_config(&cfg_path, &cfg.paths.path);
    if !sprites_root.is_dir() {
        bail!(
            "paths.path is not a directory: {} (set it to the folder containing your sprite subfolders)",
            sprites_root.display()
        );
    }

    let sheet_out = resolve_against_config(&cfg_path, &cfg.paths.sheet_output_path);

    let data_out = cfg
        .paths
        .data_output_path
        .as_deref()
        .map(|p| resolve_against_config(&cfg_path, p));

    let nesting_keys = if cfg.paths.require_path.is_some() {
        let cfg_dir = cfg_path.parent().unwrap_or_else(|| std::path::Path::new("."));
        match find_asphalt_toml(cfg_dir) {
            Some(asphalt_path) => {
                let module_name = cfg
                    .paths
                    .require_path
                    .as_deref()
                    .and_then(|r| {
                        let norm = normalize_require_path(r);
                        module_local_name_from_require_path(&norm).ok()
                    })
                    .unwrap_or_default();
                if module_name.is_empty() {
                    Vec::new()
                } else {
                    match compute_nesting_keys(
                        &asphalt_path,
                        &module_name,
                        &cfg.paths.sheet_output_path,
                    ) {
                        Ok(keys) => {
                            if !keys.is_empty() {
                                log::info!(
                                    "asphalt nesting detected: {}{}",
                                    module_name,
                                    keys.iter()
                                        .map(|k| format!("[\"{k}\"]"))
                                        .collect::<String>()
                                );
                            }
                            keys
                        }
                        Err(e) => {
                            log::warn!("could not read asphalt.toml for nesting detection: {e:#}");
                            Vec::new()
                        }
                    }
                }
            }
            None => Vec::new(),
        }
    } else {
        Vec::new()
    };

    let require = resolve_require(cfg.paths.require_path.as_deref(), nesting_keys)?;

    if data_out.is_none() {
        log::warn!("paths.data_output_path is not set — no .luau data files will be written");
    }
    if require.is_none() {
        log::warn!(
            "paths.require_path is not set — sheet entries will be plain strings instead of require() references"
        );
    }

    let cell_cli = validate_cell_pair(
        "CLI (--cell-width / --cell-height)",
        cell_width,
        cell_height,
    )?;

    let folders = discover_sprite_set_folders(&sprites_root)?;
    if folders.is_empty() {
        bail!(
            "no sprite folders found under {} (add PNG/JPEG files in a subfolder, or in this folder)",
            sprites_root.display()
        );
    }

    let mut choices: Vec<(String, PathBuf)> = Vec::new();
    for p in folders {
        let label = folder_label_relative(&sprites_root, &p)?;
        choices.push((label, p));
    }

    let selected = if let Some(name) = folder {
        let name_norm = name.trim().replace('\\', "/");
        choices
            .iter()
            .find(|(label, _)| label == &name_norm)
            .map(|(_, p)| p.clone())
            .or_else(|| {
                let rel = name_norm.replace('/', std::path::MAIN_SEPARATOR_STR);
                let wanted = sprites_root.join(rel);
                choices
                    .iter()
                    .find(|(_, p)| p.as_path() == wanted.as_path())
                    .map(|(_, p)| p.clone())
            })
            .with_context(|| {
                format!(
                    "folder {:?} not found under {} (run `spritepack run` for the full list)",
                    name,
                    sprites_root.display()
                )
            })?
    } else {
        let labels: Vec<String> = choices.iter().map(|(l, _)| l.clone()).collect();
        let prompt = format!(
            "Choose a spritesheet folder (under {})",
            cfg.paths.path.trim_end_matches(['/', '\\'])
        );
        let choice = Select::new(&prompt, labels)
            .with_page_size(20)
            .prompt()
            .context("folder selection")?;
        choices
            .into_iter()
            .find(|(l, _)| l == &choice)
            .map(|(_, p)| p)
            .context("internal: selected folder missing")?
    };

    let mut cell_width = cell_cli.map(|(w, _)| w);
    let mut cell_height = cell_cli.map(|(_, h)| h);

    if cell_width.is_none() && cell_height.is_none() && io::stdin().is_terminal() {
        let (w, h) = prompt_cell_size(MAX_SHEET_WIDTH, MAX_SHEET_HEIGHT)?;
        cell_width = Some(w);
        cell_height = Some(h);
    }

    validate_cell_pair("resolved cell size", cell_width, cell_height)?;

    let opts = PackOptions {
        cell_width,
        cell_height,
    };

    log::info!(
        "packing {} → sheets: {}{}",
        selected.display(),
        sheet_out.display(),
        match &data_out {
            Some(d) => format!(", data: {}", d.display()),
            None => String::new(),
        }
    );

    let result = pack_folder(
        &selected,
        &sheet_out,
        data_out.as_deref(),
        require.as_ref(),
        opts,
    )?;

    for p in &result.sheet_paths {
        log::info!("wrote {}", p.display());
    }
    if let Some(dp) = &result.data_path {
        log::info!("wrote {}", dp.display());
    }

    Ok(())
}

fn main() {
    init_logging();

    let cli = Cli::parse();
    let code = match &cli.command {
        Commands::Run {
            folder,
            cell_width,
            cell_height,
        } => match run_command(
            cli.config.clone(),
            folder.clone(),
            *cell_width,
            *cell_height,
        ) {
            Ok(()) => 0,
            Err(e) => {
                error!("{:#}", e);
                1
            }
        },
    };

    std::process::exit(code);
}
