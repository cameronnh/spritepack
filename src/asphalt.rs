use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct AsphaltConfig {
    #[serde(default)]
    codegen: Option<AsphaltCodegen>,
    #[serde(default)]
    inputs: HashMap<String, AsphaltInput>,
}

#[derive(Debug, Deserialize)]
struct AsphaltCodegen {
    #[serde(default)]
    style: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AsphaltInput {
    path: String,
}

/// Extract the base directory from an Asphalt glob path.
/// E.g. `"Assets/Images/**/*"` → `"Assets/Images"`.
fn glob_base(glob_path: &str) -> String {
    let normalized = glob_path.replace('\\', "/");
    let cut = normalized
        .find("**")
        .or_else(|| normalized.find('*'))
        .unwrap_or(normalized.len());
    normalized[..cut].trim_end_matches('/').to_string()
}

/// Find `asphalt.toml` by walking up from `start`.
pub fn find_asphalt_toml(start: &Path) -> Option<PathBuf> {
    let mut cur = start.to_path_buf();
    loop {
        let candidate = cur.join("asphalt.toml");
        if candidate.is_file() {
            return Some(candidate);
        }
        cur = cur.parent()?.to_path_buf();
    }
}

/// Compute nesting keys for `require()` references by analyzing the Asphalt config.
///
/// Given the module name (last segment of the `require_path`, e.g. `"ImagesIds"`) and the
/// raw `sheet_output_path` from `spritepack.toml`, this finds the matching
/// `[inputs.<module_name>]` section in `asphalt.toml`, extracts the glob base from its
/// `path`, and returns the relative folder segments between the glob base and the sheet
/// output path.
///
/// Returns an empty `Vec` when no nesting is needed (paths match exactly, codegen style
/// is not `"nested"`, or the matching input is not found).
pub fn compute_nesting_keys(
    asphalt_config_path: &Path,
    module_name: &str,
    sheet_output_path: &str,
) -> Result<Vec<String>> {
    let raw = std::fs::read_to_string(asphalt_config_path)
        .with_context(|| format!("read {}", asphalt_config_path.display()))?;
    let cfg: AsphaltConfig = toml::from_str(&raw)
        .with_context(|| format!("parse {}", asphalt_config_path.display()))?;

    let is_nested = cfg
        .codegen
        .as_ref()
        .and_then(|c| c.style.as_deref())
        .map(|s| s == "nested")
        .unwrap_or(false);

    if !is_nested {
        return Ok(Vec::new());
    }

    let input = match cfg.inputs.get(module_name) {
        Some(input) => input,
        None => return Ok(Vec::new()),
    };

    let base = glob_base(&input.path);
    let sheet_norm = sheet_output_path
        .replace('\\', "/")
        .trim_matches('/')
        .to_string();

    if sheet_norm == base || base.is_empty() {
        return Ok(Vec::new());
    }

    let prefix = format!("{base}/");
    if let Some(relative) = sheet_norm.strip_prefix(&prefix) {
        Ok(relative
            .split('/')
            .filter(|s| !s.is_empty())
            .map(String::from)
            .collect())
    } else {
        Ok(Vec::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glob_base_strips_double_star() {
        assert_eq!(glob_base("Assets/Images/**/*"), "Assets/Images");
    }

    #[test]
    fn glob_base_strips_single_star() {
        assert_eq!(glob_base("Assets/Images/*"), "Assets/Images");
    }

    #[test]
    fn glob_base_no_glob() {
        assert_eq!(glob_base("Assets/Images"), "Assets/Images");
    }

    #[test]
    fn nesting_when_sheet_path_is_subfolder() {
        let dir = std::env::temp_dir().join("spritepack_test_asphalt_nested");
        let _ = std::fs::create_dir_all(&dir);
        let toml_path = dir.join("asphalt.toml");
        std::fs::write(
            &toml_path,
            r#"
[codegen]
style = "nested"

[inputs.ImagesIds]
path = "Assets/Images/**/*"
"#,
        )
        .unwrap();

        let keys =
            compute_nesting_keys(&toml_path, "ImagesIds", "Assets/Images/Spritesheets").unwrap();
        assert_eq!(keys, vec!["Spritesheets"]);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn no_nesting_when_paths_match() {
        let dir = std::env::temp_dir().join("spritepack_test_asphalt_match");
        let _ = std::fs::create_dir_all(&dir);
        let toml_path = dir.join("asphalt.toml");
        std::fs::write(
            &toml_path,
            r#"
[codegen]
style = "nested"

[inputs.FromAsphalt]
path = "Test/Upload/**/*"
"#,
        )
        .unwrap();

        let keys = compute_nesting_keys(&toml_path, "FromAsphalt", "Test/Upload").unwrap();
        assert!(keys.is_empty());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn no_nesting_when_style_is_flat() {
        let dir = std::env::temp_dir().join("spritepack_test_asphalt_flat");
        let _ = std::fs::create_dir_all(&dir);
        let toml_path = dir.join("asphalt.toml");
        std::fs::write(
            &toml_path,
            r#"
[codegen]
style = "flat"

[inputs.ImagesIds]
path = "Assets/Images/**/*"
"#,
        )
        .unwrap();

        let keys =
            compute_nesting_keys(&toml_path, "ImagesIds", "Assets/Images/Spritesheets").unwrap();
        assert!(keys.is_empty());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn no_nesting_when_module_not_found() {
        let dir = std::env::temp_dir().join("spritepack_test_asphalt_missing");
        let _ = std::fs::create_dir_all(&dir);
        let toml_path = dir.join("asphalt.toml");
        std::fs::write(
            &toml_path,
            r#"
[codegen]
style = "nested"

[inputs.SomethingElse]
path = "Other/**/*"
"#,
        )
        .unwrap();

        let keys =
            compute_nesting_keys(&toml_path, "ImagesIds", "Assets/Images/Spritesheets").unwrap();
        assert!(keys.is_empty());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
