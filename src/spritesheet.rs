use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

use crate::config::RequireInfo;
use image::imageops::{overlay, resize, FilterType};
use image::{open, DynamicImage, Rgba, RgbaImage};

use crate::luau::{clean_entry_name, folder_to_module_ident, render_module, sheet_asset_keys};
use crate::similarity::sort_similar_items;

/// Max width of each output atlas PNG (Roblox texture-friendly).
pub const MAX_SHEET_WIDTH: u32 = 1024;
/// Max height of each output atlas PNG.
pub const MAX_SHEET_HEIGHT: u32 = 1024;

#[derive(Debug, Clone, Copy)]
pub struct PackOptions {
    /// When both width and height are `Some`, forces cell size. When both are `None`, inferred from source images.
    pub cell_width: Option<u32>,
    pub cell_height: Option<u32>,
}

pub struct PackResult {
    pub sheet_paths: Vec<PathBuf>,
    /// `None` when `data_output_path` was not configured.
    pub data_path: Option<PathBuf>,
}

fn is_image_ext(ext: &std::ffi::OsStr) -> bool {
    ext.eq_ignore_ascii_case("png")
        || ext.eq_ignore_ascii_case("jpg")
        || ext.eq_ignore_ascii_case("jpeg")
}

/// Width and height of the first image in `source_folder` (sorted by path).
pub fn peek_source_dimensions(source_folder: &Path) -> Result<(u32, u32)> {
    let paths = collect_images(source_folder)?;
    let first = paths
        .first()
        .with_context(|| format!("no images in {}", source_folder.display()))?;
    let img = open(first).with_context(|| format!("open {}", first.display()))?;
    let w = img.width();
    let h = img.height();
    if w == 0 || h == 0 {
        bail!("invalid dimensions {}×{} for {}", w, h, first.display());
    }
    Ok((w, h))
}

fn collect_images(folder: &Path) -> Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    for entry in fs::read_dir(folder).with_context(|| format!("read_dir {}", folder.display()))? {
        let entry = entry?;
        let p = entry.path();
        if p.is_file() {
            if let Some(ext) = p.extension() {
                if is_image_ext(ext) {
                    paths.push(p);
                }
            }
        }
    }
    paths.sort();
    Ok(paths)
}

/// True if this directory contains at least one PNG/JPEG directly (not in subfolders).
pub fn folder_has_direct_images(folder: &Path) -> bool {
    collect_images(folder).is_ok_and(|v| !v.is_empty())
}

/// Recursively find every directory under `sprites_root` (including the root) that has direct image files.
pub fn discover_sprite_set_folders(sprites_root: &Path) -> Result<Vec<PathBuf>> {
    if !sprites_root.is_dir() {
        bail!(
            "sprites root is not a directory: {}",
            sprites_root.display()
        );
    }

    let mut out = Vec::new();
    visit_sprite_dirs(sprites_root, &mut out)?;
    out.sort_by(|a, b| {
        folder_label_relative(sprites_root, a)
            .unwrap_or_default()
            .cmp(&folder_label_relative(sprites_root, b).unwrap_or_default())
    });
    Ok(out)
}

fn visit_sprite_dirs(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    if folder_has_direct_images(dir) {
        out.push(dir.to_path_buf());
    }

    for entry in fs::read_dir(dir).with_context(|| format!("read_dir {}", dir.display()))? {
        let entry = entry?;
        let p = entry.path();
        if p.is_dir() {
            visit_sprite_dirs(&p, out)?;
        }
    }

    Ok(())
}

/// Display path relative to `sprites_root`, using `/` separators.
pub fn folder_label_relative(sprites_root: &Path, folder: &Path) -> Result<String> {
    let rel = folder.strip_prefix(sprites_root).with_context(|| {
        format!(
            "folder {} is not under {}",
            folder.display(),
            sprites_root.display()
        )
    })?;
    let s = rel.to_string_lossy().replace('\\', "/");
    Ok(if s.is_empty() { ".".to_string() } else { s })
}

fn load_images(paths: &[PathBuf]) -> Result<Vec<(PathBuf, DynamicImage)>> {
    let mut out = Vec::new();
    for p in paths {
        let img = open(p).with_context(|| format!("open {}", p.display()))?;
        out.push((p.clone(), img));
    }
    Ok(out)
}

fn infer_cell_size(loaded: &[(PathBuf, DynamicImage)]) -> Result<(u32, u32)> {
    let (first_path, first_img) = loaded
        .first()
        .context("no images loaded for size inference")?;
    let w0 = first_img.width();
    let h0 = first_img.height();
    if w0 == 0 || h0 == 0 {
        bail!(
            "invalid dimensions {}×{} for {}",
            w0,
            h0,
            first_path.display()
        );
    }

    for (path, img) in loaded.iter().skip(1) {
        let w = img.width();
        let h = img.height();
        if w != w0 || h != h0 {
            bail!(
                "all sprites must be the same size (expected {}×{} from {}); got {}×{} from {}",
                w0,
                h0,
                first_path.display(),
                w,
                h,
                path.display()
            );
        }
    }

    Ok((w0, h0))
}

fn resolve_cell_size(loaded: &[(PathBuf, DynamicImage)], opts: PackOptions) -> Result<(u32, u32)> {
    match (opts.cell_width, opts.cell_height) {
        (Some(w), Some(h)) => {
            if w == 0 || h == 0 {
                bail!("cell width and height must be positive");
            }
            Ok((w, h))
        }
        (None, None) => infer_cell_size(loaded),
        _ => bail!(
            "set both cell width and height, or neither (omit both to use each file's pixel size, e.g. 1024×1024)"
        ),
    }
}

/// Columns = `MAX_SHEET_WIDTH / cell_width` (floor); rows per sheet = `MAX_SHEET_HEIGHT / cell_height`.
fn compute_atlas_grid(cell_width: u32, cell_height: u32) -> Result<(u32, usize)> {
    let cols_used = MAX_SHEET_WIDTH / cell_width;
    let rows_fit = MAX_SHEET_HEIGHT / cell_height;
    if cols_used == 0 || rows_fit == 0 {
        bail!(
            "cell size {}×{} cannot fit inside max atlas {}×{}; use smaller cells",
            cell_width,
            cell_height,
            MAX_SHEET_WIDTH,
            MAX_SHEET_HEIGHT
        );
    }
    let max_per_sheet = (cols_used as usize).saturating_mul(rows_fit as usize);
    if max_per_sheet == 0 {
        bail!("computed zero sprites per sheet");
    }
    Ok((cols_used, max_per_sheet))
}

/// Remove prior `{folder_name}_<n>.png` outputs so stale sheets are not left behind.
fn remove_previous_sheet_pngs(sheet_out_dir: &Path, folder_name: &str) -> Result<()> {
    let prefix = format!("{folder_name}_");
    if !sheet_out_dir.is_dir() {
        return Ok(());
    }
    for entry in fs::read_dir(sheet_out_dir)
        .with_context(|| format!("read_dir {}", sheet_out_dir.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !name.starts_with(&prefix) || !name.ends_with(".png") {
            continue;
        }
        let mid = &name[prefix.len()..name.len() - ".png".len()];
        if mid.parse::<u32>().is_err() {
            continue;
        }
        fs::remove_file(&path).with_context(|| format!("remove {}", path.display()))?;
    }
    Ok(())
}

/// Pack all images in `source_folder` into one or more PNG sheets,
/// and optionally write a Luau data module when `data_out_dir` is provided.
pub fn pack_folder(
    source_folder: &Path,
    sheet_out_dir: &Path,
    data_out_dir: Option<&Path>,
    require: Option<&RequireInfo>,
    opts: PackOptions,
) -> Result<PackResult> {
    let image_paths = collect_images(source_folder)?;
    if image_paths.is_empty() {
        bail!("no PNG/JPEG images found in {}", source_folder.display());
    }

    let loaded = load_images(&image_paths)?;
    let (cell_width, cell_height) = resolve_cell_size(&loaded, opts)?;
    log::info!(
        "cell size {}×{} ({})",
        cell_width,
        cell_height,
        if opts.cell_width.is_some() {
            "from CLI"
        } else {
            "from source images or prompt"
        },
    );

    let sorted_items = sort_similar_items(loaded);

    let (columns_used, max_per_sheet) = compute_atlas_grid(cell_width, cell_height)?;
    let rows_fit = MAX_SHEET_HEIGHT / cell_height;
    log::info!(
        "atlas: {} columns ({}÷{}), up to {} rows ({}÷{}), ≤{} sprites per PNG",
        columns_used,
        MAX_SHEET_WIDTH,
        cell_width,
        rows_fit,
        MAX_SHEET_HEIGHT,
        cell_height,
        max_per_sheet
    );

    let chunks: Vec<_> = sorted_items
        .chunks(max_per_sheet)
        .map(|c| c.to_vec())
        .collect();

    fs::create_dir_all(sheet_out_dir)
        .with_context(|| format!("create_dir_all {}", sheet_out_dir.display()))?;
    if let Some(d) = data_out_dir {
        fs::create_dir_all(d).with_context(|| format!("create_dir_all {}", d.display()))?;
    }

    let folder_name = source_folder
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("sprites");
    remove_previous_sheet_pngs(sheet_out_dir, folder_name)?;

    let module_ident = folder_to_module_ident(folder_name);
    let sheet_names = sheet_asset_keys(folder_name, chunks.len());

    let mut sheet_paths = Vec::new();
    let mut entries: Vec<(String, u32, u32)> = Vec::new();

    for (sheet_idx, chunk) in chunks.iter().enumerate() {
        let sheet_number = sheet_idx + 1;
        let rows = chunk.len().div_ceil(columns_used as usize);
        let width = cell_width * columns_used;
        let height = cell_height * rows as u32;
        let mut atlas = RgbaImage::from_pixel(width, height, Rgba([0, 0, 0, 0]));

        for (img_index, (path, dyn_img)) in chunk.iter().enumerate() {
            let rgba = dyn_img.to_rgba8();
            let cell = resize(&rgba, cell_width, cell_height, FilterType::Triangle);
            let col = (img_index as u32) % columns_used;
            let row = (img_index as u32) / columns_used;
            let x = col * cell_width;
            let y = row * cell_height;
            overlay(&mut atlas, &cell, x.into(), y.into());

            let stem = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("sprite");
            let clean = clean_entry_name(stem);
            entries.push((clean, img_index as u32 + 1, sheet_number as u32));
        }

        let png_name = format!("{folder_name}_{sheet_number}.png");
        let sheet_path = sheet_out_dir.join(&png_name);
        atlas
            .save(&sheet_path)
            .with_context(|| format!("save {}", sheet_path.display()))?;
        sheet_paths.push(sheet_path);
    }

    let data_path = if let Some(d) = data_out_dir {
        let luau_params = crate::luau::LuauModuleParams {
            module_ident: &module_ident,
            require,
            cell_width,
            cell_height,
            columns: columns_used,
        };
        let luau_src = render_module(&luau_params, &sheet_names, &entries)?;
        let path = d.join(format!("{folder_name}.luau"));
        fs::write(&path, luau_src).with_context(|| format!("write {}", path.display()))?;
        Some(path)
    } else {
        None
    };

    Ok(PackResult {
        sheet_paths,
        data_path,
    })
}
