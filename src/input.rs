use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use serde::Deserialize;

pub type WorkspaceAliases = BTreeMap<String, BTreeMap<String, String>>;

#[derive(Debug, Clone)]
pub struct SourceFile {
    pub crate_name: String,
    pub module_path: String,
    pub relative_path: String,
    pub bytes: Vec<u8>,
}

#[derive(Debug)]
pub struct Inventory {
    pub sources: Vec<SourceFile>,
    pub workspace_aliases: WorkspaceAliases,
    pub content_digest: String,
    pub metadata_complete: bool,
    pub metadata_detail: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Metadata {
    packages: Vec<Package>,
    workspace_members: Vec<String>,
    workspace_root: String,
    target_directory: String,
}

#[derive(Debug, Deserialize)]
struct Package {
    name: String,
    id: String,
    manifest_path: String,
    targets: Vec<Target>,
    #[serde(default)]
    dependencies: Vec<Dependency>,
}

#[derive(Debug, Deserialize)]
struct Target {
    name: String,
    kind: Vec<String>,
    src_path: String,
}

#[derive(Debug, Deserialize)]
struct Dependency {
    name: String,
    rename: Option<String>,
    path: Option<String>,
}

pub fn inventory(root: &Path) -> Result<Inventory, String> {
    let root = root
        .canonicalize()
        .map_err(|error| format!("cannot resolve {}: {error}", root.display()))?;

    let metadata = load_metadata(&root);
    let (mut sources, workspace_aliases, complete, detail) = match metadata {
        Ok(metadata) => match inventory_from_metadata(&root, metadata) {
            Ok((sources, aliases)) => (sources, aliases, true, None),
            Err(error) => (
                fallback_inventory(&root)?,
                WorkspaceAliases::new(),
                false,
                Some(format!("Cargo metadata inventory failed: {error}")),
            ),
        },
        Err(error) => (
            fallback_inventory(&root)?,
            WorkspaceAliases::new(),
            false,
            Some(format!("Cargo metadata unavailable: {error}")),
        ),
    };

    sources.sort_by(|a, b| {
        (&a.crate_name, &a.module_path, &a.relative_path).cmp(&(
            &b.crate_name,
            &b.module_path,
            &b.relative_path,
        ))
    });
    sources.dedup_by(|a, b| {
        a.crate_name == b.crate_name
            && a.module_path == b.module_path
            && a.relative_path == b.relative_path
    });

    let mut hasher = blake3::Hasher::new();
    for source in &sources {
        hasher.update(source.crate_name.as_bytes());
        hasher.update(&[0]);
        hasher.update(source.module_path.as_bytes());
        hasher.update(&[0]);
        hasher.update(source.relative_path.as_bytes());
        hasher.update(&[0]);
        hasher.update(&source.bytes);
        hasher.update(&[0xff]);
    }

    for (crate_name, aliases) in &workspace_aliases {
        hasher.update(crate_name.as_bytes());
        hasher.update(&[0xfe]);
        for (alias, target) in aliases {
            hasher.update(alias.as_bytes());
            hasher.update(&[0]);
            hasher.update(target.as_bytes());
            hasher.update(&[0xfd]);
        }
    }

    Ok(Inventory {
        sources,
        workspace_aliases,
        content_digest: hasher.finalize().to_hex().to_string(),
        metadata_complete: complete,
        metadata_detail: detail,
    })
}

fn load_metadata(root: &Path) -> Result<Metadata, String> {
    let output = Command::new("cargo")
        .current_dir(root)
        .args([
            "metadata",
            "--format-version",
            "1",
            "--no-deps",
            "--offline",
            "--locked",
        ])
        .output()
        .map_err(|error| format!("could not execute cargo metadata: {error}"))?;

    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_owned());
    }

    serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("invalid cargo metadata JSON: {error}"))
}

fn inventory_from_metadata(
    root: &Path,
    metadata: Metadata,
) -> Result<(Vec<SourceFile>, WorkspaceAliases), String> {
    let metadata_root = PathBuf::from(&metadata.workspace_root);
    let target_directory = PathBuf::from(&metadata.target_directory);
    let members: BTreeSet<&str> = metadata
        .workspace_members
        .iter()
        .map(String::as_str)
        .collect();

    let packages = metadata
        .packages
        .into_iter()
        .filter(|package| members.contains(package.id.as_str()))
        .collect::<Vec<_>>();

    let mut package_roots = BTreeMap::<PathBuf, String>::new();
    let mut package_library_crates = BTreeMap::<PathBuf, String>::new();

    for package in &packages {
        let manifest = PathBuf::from(&package.manifest_path);
        let package_root = manifest
            .parent()
            .ok_or_else(|| format!("manifest has no parent directory: {}", manifest.display()))?
            .to_path_buf();

        if !package_root.starts_with(&metadata_root) || !package_root.starts_with(root) {
            continue;
        }

        package_roots.insert(package_root.clone(), package.name.clone());
        if let Some(target) = package.targets.iter().find(|target| {
            target
                .kind
                .iter()
                .any(|kind| kind == "lib" || kind == "rlib")
        }) {
            package_library_crates.insert(package_root, rust_name(&target.name));
        }
    }

    let mut crate_roots = BTreeMap::<PathBuf, String>::new();
    let mut crate_package_roots = BTreeMap::<String, PathBuf>::new();

    for package in &packages {
        let manifest = PathBuf::from(&package.manifest_path);
        let package_root = manifest
            .parent()
            .ok_or_else(|| format!("manifest has no parent directory: {}", manifest.display()))?;

        if !package_roots.contains_key(package_root) {
            continue;
        }

        for target in &package.targets {
            if !target
                .kind
                .iter()
                .any(|kind| kind == "lib" || kind == "rlib" || kind == "bin")
            {
                continue;
            }

            let source = PathBuf::from(&target.src_path);
            if let Some(src_dir) = production_source_root(package_root, &source) {
                let crate_name = rust_name(&target.name);
                crate_roots
                    .entry(src_dir)
                    .or_insert_with(|| crate_name.clone());
                crate_package_roots
                    .entry(crate_name)
                    .or_insert_with(|| package_root.to_path_buf());
            }
        }
    }

    let mut aliases = WorkspaceAliases::new();
    for (crate_name, package_root) in &crate_package_roots {
        let Some(package) = packages.iter().find(|package| {
            Path::new(&package.manifest_path).parent() == Some(package_root.as_path())
        }) else {
            continue;
        };

        let mut crate_aliases = BTreeMap::new();
        for dependency in &package.dependencies {
            let Some(path) = &dependency.path else {
                continue;
            };
            let dependency_root = PathBuf::from(path);
            let Some(target_crate) = package_library_crates.get(&dependency_root) else {
                continue;
            };
            let alias = dependency
                .rename
                .as_deref()
                .unwrap_or(&dependency.name);
            crate_aliases.insert(rust_name(alias), target_crate.clone());
        }

        if let Some(library_crate) = package_library_crates.get(package_root) {
            if library_crate != crate_name {
                crate_aliases.insert(library_crate.clone(), library_crate.clone());
            }
        }

        aliases.insert(crate_name.clone(), crate_aliases);
    }

    let mut sources = Vec::new();
    for (src_root, crate_name) in crate_roots {
        collect_rust_files(
            root,
            &src_root,
            &target_directory,
            &crate_name,
            &mut sources,
        )?;
    }

    Ok((sources, aliases))
}

fn production_source_root(package_root: &Path, target_source: &Path) -> Option<PathBuf> {
    let src = package_root.join("src");
    target_source.starts_with(&src).then_some(src)
}

fn fallback_inventory(root: &Path) -> Result<Vec<SourceFile>, String> {
    let mut sources = Vec::new();
    let target = root.join("target");
    collect_rust_files(root, root, &target, "unknown", &mut sources)?;
    Ok(sources)
}

fn collect_rust_files(
    repo_root: &Path,
    directory: &Path,
    target_directory: &Path,
    crate_name: &str,
    out: &mut Vec<SourceFile>,
) -> Result<(), String> {
    let entries = fs::read_dir(directory)
        .map_err(|error| format!("cannot read {}: {error}", directory.display()))?;

    for entry in entries {
        let entry = entry.map_err(|error| format!("cannot read directory entry: {error}"))?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .map_err(|error| format!("cannot inspect {}: {error}", path.display()))?;

        if file_type.is_symlink() {
            continue;
        }

        if file_type.is_dir() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if path == target_directory
                || name == ".git"
                || name == ".ferric-lens"
                || name == "tests"
                || name == "benches"
                || name == "examples"
            {
                continue;
            }
            collect_rust_files(repo_root, &path, target_directory, crate_name, out)?;
            continue;
        }

        if path.extension().and_then(|value| value.to_str()) != Some("rs") {
            continue;
        }

        let metadata = fs::metadata(&path)
            .map_err(|error| format!("cannot inspect {}: {error}", path.display()))?;
        if metadata.len() > 8 * 1024 * 1024 {
            continue;
        }

        let bytes =
            fs::read(&path).map_err(|error| format!("cannot read {}: {error}", path.display()))?;
        let relative = path
            .strip_prefix(repo_root)
            .map_err(|_| format!("source escaped repository: {}", path.display()))?;
        let relative_path = slash_path(relative);
        let module_path = module_path_from_relative(&relative_path);

        out.push(SourceFile {
            crate_name: crate_name.to_owned(),
            module_path,
            relative_path,
            bytes,
        });
    }
    Ok(())
}

fn rust_name(value: &str) -> String {
    value.replace('-', "_")
}

fn slash_path(path: &Path) -> String {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

fn module_path_from_relative(relative: &str) -> String {
    let mut components: Vec<&str> = relative.split('/').collect();
    let src_index = components.iter().rposition(|part| *part == "src");
    if let Some(index) = src_index {
        components = components.split_off(index + 1);
    }

    let last = components.pop().unwrap_or("lib.rs");
    let stem = last.strip_suffix(".rs").unwrap_or(last);
    if stem != "lib" && stem != "main" && stem != "mod" {
        components.push(stem);
    }

    components.join("::")
}

#[cfg(test)]
mod tests {
    use super::{module_path_from_relative, rust_name};

    #[test]
    fn derives_module_paths_from_common_layouts() {
        assert_eq!(module_path_from_relative("src/lib.rs"), "");
        assert_eq!(module_path_from_relative("src/foo.rs"), "foo");
        assert_eq!(module_path_from_relative("src/foo/mod.rs"), "foo");
        assert_eq!(module_path_from_relative("src/foo/bar.rs"), "foo::bar");
        assert_eq!(module_path_from_relative("crates/a/src/x.rs"), "x");
    }

    #[test]
    fn normalizes_cargo_names_for_rust_imports() {
        assert_eq!(rust_name("jeko-core"), "jeko_core");
        assert_eq!(rust_name("custom_name"), "custom_name");
    }
}
