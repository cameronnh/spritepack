use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct SpritepackConfig {
    pub paths: Paths,
}

#[derive(Debug, Deserialize)]
pub struct Paths {
    /// Root directory scanned for sprite sets (subfolders with images, recursively).
    pub path: String,
    /// Output directory for generated PNG spritesheets (e.g. Asphalt input folder).
    pub sheet_output_path: String,
    /// Optional output directory for generated `.luau` data modules.
    /// When absent, no Luau files are written.
    #[serde(default)]
    pub data_output_path: Option<String>,
    /// Optional Rojo `require()` path. When set, generated modules emit
    /// `local X = require("…")` and bind sheet values as `X["key"]`.
    /// When absent, sheets are plain strings like `"Brainrots_1"`.
    #[serde(default)]
    pub require_path: Option<String>,
}

/// Normalize the user's `require_path`: trim, strip optional `.luau` / `.lua`, normalize separators.
pub fn normalize_require_path(raw: &str) -> String {
    let t = raw.trim();
    let rest = t.strip_prefix('@').unwrap_or(t).trim();
    let rest = rest
        .strip_suffix(".luau")
        .or_else(|| rest.strip_suffix(".lua"))
        .unwrap_or(rest);
    let rest = rest.replace('\\', "/").trim_matches('/').to_string();
    format!("@{rest}")
}

fn is_valid_luau_identifier(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Last `/`-separated segment of a normalized `require_path`, e.g.
/// `@game/ReplicatedStorage/Test/FromAsphalt` → `FromAsphalt`.
pub fn module_local_name_from_require_path(require_path: &str) -> anyhow::Result<String> {
    let last = require_path
        .trim_start_matches('@')
        .rsplit('/')
        .find(|s| !s.is_empty())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "could not derive a module name from require_path {:?}",
                require_path
            )
        })?;
    if !is_valid_luau_identifier(last) {
        anyhow::bail!(
            "require_path's last segment must be a valid Luau identifier: got {:?} from {:?}",
            last,
            require_path
        );
    }
    Ok(last.to_string())
}

/// Resolved require info passed to the Luau renderer.
/// `None` means no require — sheets are plain strings.
#[derive(Debug, Clone)]
pub struct RequireInfo {
    /// Full `require()` path, e.g. `@game/ReplicatedStorage/Test/FromAsphalt`.
    pub require_path: String,
    /// Luau local name derived from `require_path`, e.g. `FromAsphalt`.
    pub local_name: String,
    /// Intermediate table keys between the module local and the sheet key.
    /// E.g. `["Spritesheets"]` produces `ImagesIds["Spritesheets"]["Sheet_1"]`.
    pub nesting_keys: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_require_path_strips_ext_keeps_slashes() {
        assert_eq!(
            normalize_require_path("@Test/FromAsphalt.luau"),
            "@Test/FromAsphalt"
        );
    }

    #[test]
    fn normalize_require_path_preserves_game_style_path() {
        assert_eq!(
            normalize_require_path("@game/ReplicatedStorage/Test/FromAsphalt"),
            "@game/ReplicatedStorage/Test/FromAsphalt"
        );
    }

    #[test]
    fn module_local_name_uses_last_segment() {
        let out =
            module_local_name_from_require_path("@game/ReplicatedStorage/Test/SomeOtherFileName")
                .unwrap();
        assert_eq!(out, "SomeOtherFileName");
    }

    #[test]
    fn module_local_name_rejects_invalid_identifier() {
        assert!(module_local_name_from_require_path("@game/ReplicatedStorage/123Bad").is_err());
    }
}
