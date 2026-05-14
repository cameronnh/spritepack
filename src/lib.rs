mod config;
mod luau;
mod similarity;
mod spritesheet;

pub use config::{
    module_local_name_from_require_path, normalize_require_path, Paths, RequireInfo,
    SpritepackConfig,
};
pub use luau::LuauModuleParams;
pub use spritesheet::{
    discover_sprite_set_folders, folder_label_relative, pack_folder, peek_source_dimensions,
    PackOptions, PackResult, MAX_SHEET_HEIGHT, MAX_SHEET_WIDTH,
};

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// Locate `spritepack.toml` by walking up from `start` (typically the current directory).
pub fn find_spritepack_toml(start: &Path) -> Option<PathBuf> {
    let mut cur = start.to_path_buf();
    loop {
        let candidate = cur.join("spritepack.toml");
        if candidate.is_file() {
            return Some(candidate);
        }
        cur = cur.parent()?.to_path_buf();
    }
}

/// Load and parse `spritepack.toml` at the given path.
pub fn load_config(path: &Path) -> Result<SpritepackConfig> {
    let raw = std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    toml::from_str(&raw).with_context(|| format!("parse {}", path.display()))
}

/// Resolve `relative` against the directory containing the config file.
pub fn resolve_against_config(config_path: &Path, relative: &str) -> PathBuf {
    let base = config_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf();
    base.join(relative)
}
