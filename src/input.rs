use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use serde::Deserialize;
use syn::{Attribute, Item, ItemMod};

use crate::{cfg::Truth, profile::ProfileContext};

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
    inventory_impl(root, None)
}

pub fn inventory_with_profile(root: &Path, profile: &ProfileContext) -> Result<Inventory, String> {
    inventory_impl(root, Some(profile))
}

fn inventory_impl(root: &Path, profile: Option<&ProfileContext>) -> Result<Inventory, String> {
    let root = root
        .canonicalize()
        .map_err(|error| format!("cannot resolve {}: {error}", root.display()))?;

    let metadata = load_metadata(&root, profile);
    let (mut sources, workspace_aliases, complete, detail) = match metadata {
        Ok(metadata) => match inventory_from_metadata(&root, metadata, profile) {
            Ok((sources, aliases, limitations)) => {
                let complete = limitations.is_empty();
                let detail = summarize_limitations(&limitations);
                (sources, aliases, complete, detail)
            }
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

fn load_metadata(root: &Path, profile: Option<&ProfileContext>) -> Result<Metadata, String> {
    let mut command = Command::new("cargo");
    command.current_dir(root).args([
        "metadata",
        "--format-version",
        "1",
        "--no-deps",
        "--offline",
        "--locked",
    ]);

    if let Some(profile) = profile {
        command
            .arg("--filter-platform")
            .arg(&profile.public.resolved_target);
        if !profile.public.features.is_empty() {
            command
                .arg("--features")
                .arg(profile.public.features.join(","));
        }
    }

    let output = command
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
    profile: Option<&ProfileContext>,
) -> Result<(Vec<SourceFile>, WorkspaceAliases, Vec<String>), String> {
    let metadata_root = PathBuf::from(&metadata.workspace_root);
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

    let mut target_roots = Vec::<(String, PathBuf, PathBuf)>::new();
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
            if !source.starts_with(package_root)
                || !source.starts_with(root)
                || source.extension().and_then(|value| value.to_str()) != Some("rs")
            {
                continue;
            }

            let crate_name = rust_name(&target.name);
            target_roots.push((crate_name.clone(), source, package_root.to_path_buf()));
            crate_package_roots
                .entry(crate_name)
                .or_insert_with(|| package_root.to_path_buf());
        }
    }

    target_roots.sort_by(|left, right| (&left.0, &left.1).cmp(&(&right.0, &right.1)));

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
            let alias = dependency.rename.as_deref().unwrap_or(&dependency.name);
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
    let mut limitations = Vec::new();

    for (crate_name, source, _package_root) in target_roots {
        let mut visited = BTreeSet::new();
        collect_reachable_module(
            root,
            &crate_name,
            &source,
            "",
            true,
            profile,
            &mut visited,
            &mut sources,
            &mut limitations,
        )?;
    }

    limitations.sort();
    limitations.dedup();
    Ok((sources, aliases, limitations))
}

#[allow(clippy::too_many_arguments)]
fn collect_reachable_module(
    repo_root: &Path,
    crate_name: &str,
    source_path: &Path,
    module_path: &str,
    is_crate_root: bool,
    profile: Option<&ProfileContext>,
    visited: &mut BTreeSet<PathBuf>,
    out: &mut Vec<SourceFile>,
    limitations: &mut Vec<String>,
) -> Result<(), String> {
    let metadata = match fs::symlink_metadata(source_path) {
        Ok(metadata) => metadata,
        Err(error) => {
            limitations.push(format!(
                "{crate_name}: module source {} is unavailable: {error}",
                source_path.display()
            ));
            return Ok(());
        }
    };

    if metadata.file_type().is_symlink() {
        limitations.push(format!(
            "{crate_name}: module source {} is a symlink and was not followed",
            source_path.display()
        ));
        return Ok(());
    }

    if metadata.len() > 8 * 1024 * 1024 {
        limitations.push(format!(
            "{crate_name}: module source {} exceeds the 8 MiB source limit",
            source_path.display()
        ));
        return Ok(());
    }

    let canonical = source_path
        .canonicalize()
        .map_err(|error| format!("cannot resolve {}: {error}", source_path.display()))?;
    if !canonical.starts_with(repo_root) {
        limitations.push(format!(
            "{crate_name}: module source {} resolves outside the repository",
            source_path.display()
        ));
        return Ok(());
    }

    if !visited.insert(canonical.clone()) {
        return Ok(());
    }

    let bytes = fs::read(&canonical)
        .map_err(|error| format!("cannot read {}: {error}", canonical.display()))?;
    let relative = canonical
        .strip_prefix(repo_root)
        .map_err(|_| format!("source escaped repository: {}", canonical.display()))?;
    let relative_path = slash_path(relative);

    out.push(SourceFile {
        crate_name: crate_name.to_owned(),
        module_path: module_path.to_owned(),
        relative_path,
        bytes: bytes.clone(),
    });

    let text = match std::str::from_utf8(&bytes) {
        Ok(text) => text,
        Err(error) => {
            limitations.push(format!(
                "{crate_name}: cannot discover child modules from {} because it is not UTF-8: {error}",
                canonical.display()
            ));
            return Ok(());
        }
    };
    let syntax = match syn::parse_file(text) {
        Ok(syntax) => syntax,
        Err(error) => {
            limitations.push(format!(
                "{crate_name}: cannot discover child modules from {}: {error}",
                canonical.display()
            ));
            return Ok(());
        }
    };

    let parent = canonical
        .parent()
        .ok_or_else(|| format!("module source has no parent: {}", canonical.display()))?;
    let module_dir = if is_crate_root
        || canonical.file_name().and_then(|name| name.to_str()) == Some("mod.rs")
    {
        parent.to_path_buf()
    } else {
        let stem = canonical
            .file_stem()
            .ok_or_else(|| format!("module source has no stem: {}", canonical.display()))?;
        parent.join(stem)
    };

    discover_child_modules(
        repo_root,
        crate_name,
        &syntax.items,
        module_path,
        &module_dir,
        profile,
        visited,
        out,
        limitations,
    )
}

#[allow(clippy::too_many_arguments)]
fn discover_child_modules(
    repo_root: &Path,
    crate_name: &str,
    items: &[Item],
    parent_module: &str,
    module_dir: &Path,
    profile: Option<&ProfileContext>,
    visited: &mut BTreeSet<PathBuf>,
    out: &mut Vec<SourceFile>,
    limitations: &mut Vec<String>,
) -> Result<(), String> {
    for item in items {
        let Item::Mod(module) = item else {
            continue;
        };

        if module
            .attrs
            .iter()
            .any(|attr| attr.path().is_ident("cfg_attr"))
        {
            limitations.push(format!(
                "{crate_name}: module {} uses cfg_attr and reachability is incomplete",
                child_module_path(parent_module, module)
            ));
            continue;
        }

        match module_cfg_state(&module.attrs, profile) {
            Truth::False => continue,
            Truth::Unknown => {
                limitations.push(format!(
                    "{crate_name}: module {} has unresolved cfg reachability",
                    child_module_path(parent_module, module)
                ));
                continue;
            }
            Truth::True => {}
        }

        if module.attrs.iter().any(|attr| attr.path().is_ident("path")) {
            limitations.push(format!(
                "{crate_name}: module {} uses #[path] and reachability is incomplete",
                child_module_path(parent_module, module)
            ));
            continue;
        }

        let child_path = child_module_path(parent_module, module);
        if let Some((_, inline_items)) = &module.content {
            let inline_dir = module_dir.join(module.ident.to_string());
            discover_child_modules(
                repo_root,
                crate_name,
                inline_items,
                &child_path,
                &inline_dir,
                profile,
                visited,
                out,
                limitations,
            )?;
            continue;
        }

        let ident = module.ident.to_string();
        let flat = module_dir.join(format!("{ident}.rs"));
        let nested = module_dir.join(&ident).join("mod.rs");
        let flat_exists = flat.is_file() || flat.is_symlink();
        let nested_exists = nested.is_file() || nested.is_symlink();

        let source = match (flat_exists, nested_exists) {
            (true, false) => flat,
            (false, true) => nested,
            (false, false) => {
                limitations.push(format!(
                    "{crate_name}: module {child_path} has no discoverable source file"
                ));
                continue;
            }
            (true, true) => {
                limitations.push(format!(
                    "{crate_name}: module {child_path} has ambiguous source files"
                ));
                continue;
            }
        };

        collect_reachable_module(
            repo_root,
            crate_name,
            &source,
            &child_path,
            false,
            profile,
            visited,
            out,
            limitations,
        )?;
    }

    Ok(())
}

fn child_module_path(parent: &str, module: &ItemMod) -> String {
    if parent.is_empty() {
        module.ident.to_string()
    } else {
        format!("{parent}::{}", module.ident)
    }
}

fn module_cfg_state(attrs: &[Attribute], profile: Option<&ProfileContext>) -> Truth {
    let Some(profile) = profile else {
        return Truth::True;
    };

    let mut state = Truth::True;
    for attr in attrs.iter().filter(|attr| attr.path().is_ident("cfg")) {
        let Ok(meta) = attr.parse_args::<syn::Meta>() else {
            return Truth::Unknown;
        };
        state = match (state, profile.cfg.evaluate(&meta)) {
            (Truth::False, _) | (_, Truth::False) => Truth::False,
            (Truth::Unknown, _) | (_, Truth::Unknown) => Truth::Unknown,
            (Truth::True, Truth::True) => Truth::True,
        };
        if state == Truth::False {
            break;
        }
    }
    state
}

fn summarize_limitations(limitations: &[String]) -> Option<String> {
    if limitations.is_empty() {
        return None;
    }

    let shown = limitations.iter().take(3).cloned().collect::<Vec<_>>();
    let suffix = if limitations.len() > shown.len() {
        format!(
            "; {} additional inventory limitation(s)",
            limitations.len() - shown.len()
        )
    } else {
        String::new()
    };
    Some(format!("{}{}", shown.join("; "), suffix))
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
    use std::{
        collections::BTreeSet,
        fs,
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
    };

    use super::{collect_reachable_module, module_path_from_relative, rust_name};

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn temp_root() -> PathBuf {
        let counter = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "ferric-lens-input-test-{}-{counter}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("src")).unwrap();
        root
    }

    #[test]
    fn derives_module_paths_from_common_layouts() {
        assert_eq!(module_path_from_relative("src/lib.rs"), "");
        assert_eq!(module_path_from_relative("src/foo.rs"), "foo");
        assert_eq!(module_path_from_relative("src/foo/mod.rs"), "foo");
        assert_eq!(module_path_from_relative("src/foo/bar.rs"), "foo::bar");
        assert_eq!(module_path_from_relative("crates/a/src/x.rs"), "x");
    }

    #[test]
    fn target_inventory_excludes_orphan_rust_files() {
        let root = temp_root();
        fs::write(root.join("src/lib.rs"), "mod used;").unwrap();
        fs::write(root.join("src/used.rs"), "pub fn used() {}").unwrap();
        fs::write(root.join("src/orphan.rs"), "pub fn orphan() {}").unwrap();

        let mut visited = BTreeSet::new();
        let mut sources = Vec::new();
        let mut limitations = Vec::new();
        collect_reachable_module(
            &root,
            "demo",
            &root.join("src/lib.rs"),
            "",
            true,
            None,
            &mut visited,
            &mut sources,
            &mut limitations,
        )
        .unwrap();

        sources.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
        assert_eq!(
            sources
                .iter()
                .map(|source| source.relative_path.as_str())
                .collect::<Vec<_>>(),
            vec!["src/lib.rs", "src/used.rs"]
        );
        assert!(limitations.is_empty());

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn discovers_file_modules_nested_under_inline_modules() {
        let root = temp_root();
        fs::create_dir_all(root.join("src/outer")).unwrap();
        fs::write(root.join("src/lib.rs"), "mod outer { mod child; }").unwrap();
        fs::write(root.join("src/outer/child.rs"), "pub fn child() {}").unwrap();

        let mut visited = BTreeSet::new();
        let mut sources = Vec::new();
        let mut limitations = Vec::new();
        collect_reachable_module(
            &root,
            "demo",
            &root.join("src/lib.rs"),
            "",
            true,
            None,
            &mut visited,
            &mut sources,
            &mut limitations,
        )
        .unwrap();

        assert!(sources.iter().any(|source| {
            source.relative_path == "src/outer/child.rs" && source.module_path == "outer::child"
        }));
        assert!(limitations.is_empty());

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn normalizes_cargo_names_for_rust_imports() {
        assert_eq!(rust_name("jeko-core"), "jeko_core");
        assert_eq!(rust_name("custom_name"), "custom_name");
    }
}
