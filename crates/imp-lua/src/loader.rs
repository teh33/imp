use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::sandbox::{LuaError, LuaRuntime};

/// Discovered Lua extension.
#[derive(Debug, Clone)]
pub struct LuaExtension {
    pub name: String,
    pub path: PathBuf,
    pub manifest: LuaExtensionManifest,
}

/// Optional extension metadata loaded from `manifest.lua` beside `init.lua`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LuaExtensionManifest {
    pub name: String,
    pub version: Option<String>,
    pub description: Option<String>,
    pub commands: Vec<LuaManifestCommand>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LuaManifestCommand {
    pub name: String,
    pub description: Option<String>,
}

impl LuaExtensionManifest {
    fn inferred(name: String) -> Self {
        Self {
            name,
            version: None,
            description: None,
            commands: Vec::new(),
        }
    }
}

/// Discover Lua extensions from user and project directories.
pub fn discover_extensions(
    user_config_dir: &Path,
    project_dir: Option<&Path>,
) -> Vec<LuaExtension> {
    let mut extensions = Vec::new();

    let mut dirs = vec![user_config_dir.join("lua")];
    if let Some(project) = project_dir {
        dirs.push(project.join(".imp").join("lua"));
    }

    let mut seen_names = BTreeSet::new();
    for dir in &dirs {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();

                // Direct .lua file
                if path.extension().is_some_and(|e| e == "lua") {
                    let name = path
                        .file_stem()
                        .map(|s| s.to_string_lossy().to_string())
                        .unwrap_or_default();
                    if seen_names.insert(name.clone()) {
                        extensions.push(LuaExtension {
                            name: name.clone(),
                            path,
                            manifest: LuaExtensionManifest::inferred(name),
                        });
                    }
                    continue;
                }

                // Directory with init.lua
                if path.is_dir() {
                    let init = path.join("init.lua");
                    if init.exists() {
                        let inferred_name = path
                            .file_name()
                            .map(|s| s.to_string_lossy().to_string())
                            .unwrap_or_default();
                        let manifest = load_manifest(&path)
                            .unwrap_or_else(|| LuaExtensionManifest::inferred(inferred_name));
                        if seen_names.insert(manifest.name.clone()) {
                            extensions.push(LuaExtension {
                                name: manifest.name.clone(),
                                path: init,
                                manifest,
                            });
                        }
                    }
                }
            }
        }
    }

    extensions
}

fn load_manifest(extension_dir: &Path) -> Option<LuaExtensionManifest> {
    let path = extension_dir.join("manifest.lua");
    if !path.exists() {
        return None;
    }

    let source = std::fs::read_to_string(path).ok()?;
    let lua = mlua::Lua::new();
    let value: mlua::Value = lua.load(&source).eval().ok()?;
    manifest_from_lua_value(value)
}

fn manifest_from_lua_value(value: mlua::Value) -> Option<LuaExtensionManifest> {
    let table = match value {
        mlua::Value::Table(table) => table,
        _ => return None,
    };
    let name = table.get::<String>("name").ok()?.trim().to_string();
    if name.is_empty() {
        return None;
    }

    let version = table
        .get::<Option<String>>("version")
        .ok()
        .flatten()
        .filter(|value| !value.trim().is_empty());
    let description = table
        .get::<Option<String>>("description")
        .ok()
        .flatten()
        .filter(|value| !value.trim().is_empty());
    let commands = table
        .get::<Option<mlua::Table>>("commands")
        .ok()
        .flatten()
        .map(manifest_commands_from_table)
        .unwrap_or_default();

    Some(LuaExtensionManifest {
        name,
        version,
        description,
        commands,
    })
}

fn manifest_commands_from_table(commands: mlua::Table) -> Vec<LuaManifestCommand> {
    commands
        .sequence_values::<mlua::Table>()
        .filter_map(Result::ok)
        .filter_map(|command| {
            let name = command.get::<String>("name").ok()?.trim().to_string();
            if name.is_empty() {
                return None;
            }
            let description = command
                .get::<Option<String>>("description")
                .ok()
                .flatten()
                .filter(|value| !value.trim().is_empty());
            Some(LuaManifestCommand { name, description })
        })
        .collect()
}

/// Load all discovered extensions into a Lua runtime.
pub fn load_extensions(
    runtime: &LuaRuntime,
    extensions: &[LuaExtension],
) -> Vec<(String, Result<(), LuaError>)> {
    extensions
        .iter()
        .map(|ext| {
            runtime.set_current_extension(Some(ext.name.clone()));
            let result = runtime.exec_file(&ext.path);
            runtime.set_current_extension(None);
            (ext.name.clone(), result)
        })
        .collect()
}

/// Hot reload: drop old state, create new runtime, re-load extensions.
pub fn reload(
    user_config_dir: &Path,
    project_dir: Option<&Path>,
    policy: &imp_core::config::LuaCapabilityPolicy,
) -> Result<(LuaRuntime, Vec<LuaExtension>), LuaError> {
    let extensions = discover_extensions(user_config_dir, project_dir);
    let runtime = LuaRuntime::new()?;
    crate::bridge::setup_host_api(&runtime)?;
    runtime.apply_capability_policy(policy);
    load_extensions(&runtime, &extensions);
    Ok((runtime, extensions))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discover_extensions_deduplicates_global_and_project_names() {
        let temp = tempfile::tempdir().unwrap();
        let user_config = temp.path().join("user");
        let project = temp.path().join("project");
        std::fs::create_dir_all(user_config.join("lua")).unwrap();
        std::fs::create_dir_all(project.join(".imp").join("lua")).unwrap();
        std::fs::write(user_config.join("lua").join("imp-update.lua"), "").unwrap();
        std::fs::write(project.join(".imp").join("lua").join("imp-update.lua"), "").unwrap();

        let extensions = discover_extensions(&user_config, Some(&project));

        assert_eq!(extensions.len(), 1);
        assert_eq!(extensions[0].name, "imp-update");
        assert_eq!(
            extensions[0].path,
            user_config.join("lua").join("imp-update.lua")
        );
    }
}
