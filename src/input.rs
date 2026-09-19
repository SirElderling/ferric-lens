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

const MAX_SOURCE_FILE_BYTES: u64 = 8 * 1024 * 1024;
const MAX_SNAPSHOT_SOURCE_BYTES: u64 = 512 * 1024 * 1024;

#[derive(Debug, Default)]
struct SourceBudget {
    used: u64,
    exhausted: bool,
}

impl SourceBudget {
    fn reserve(&mut self, bytes: u64) -> bool {
        if self.exhausted {
            return false;
        }
        match self.used.checked_add(bytes) {
            Some(total) if total <= MAX_SNAPSHOT_SOURCE_BYTES => {
                self.used = total;
                true
            }
            _ => {
                self.exhausted = true;
                false
            }
        }
    }
}

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

#[derive(Debug, Deserialize, Clone)]
struct Dependency {
    name: String,
    rename: Option<String>,
    path: Option<String>,
}

#[derive(Clone)]
struct TargetRoot {
    id: String,
    import_name: String,
    kind: &'static str,
    source: PathBuf,
    package_root: PathBuf,
    dependencies: Vec<Dependency>,
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

    let acquired = acquire_inventory(&root, load_metadata(&root, profile), profile)?;
    Ok(finalize_inventory(acquired))
}

type AcquiredInventory = (Vec<SourceFile>, WorkspaceAliases, bool, Option<String>);

fn acquire_inventory(
    root: &Path,
    metadata: Result<Metadata, String>,
    profile: Option<&ProfileContext>,
) -> Result<AcquiredInventory, String> {
    match metadata {
        Ok(metadata) => match inventory_from_metadata(root, metadata, profile) {
            Ok((sources, aliases, limitations)) => Ok((
                sources,
                aliases,
                limitations.is_empty(),
                summarize_limitations(&limitations),
            )),
            Err(error) => {
                let (sources, limitations) = fallback_inventory(root)?;
                Ok((
                    sources,
                    WorkspaceAliases::new(),
                    false,
                    fallback_detail(
                        &format!("Cargo metadata inventory failed: {error}"),
                        &limitations,
                    ),
                ))
            }
        },
        Err(error) => {
            let (sources, limitations) = fallback_inventory(root)?;
            Ok((
                sources,
                WorkspaceAliases::new(),
                false,
                fallback_detail(
                    &format!("Cargo metadata unavailable: {error}"),
                    &limitations,
                ),
            ))
        }
    }
}

fn finalize_inventory(acquired: AcquiredInventory) -> Inventory {
    let (mut sources, workspace_aliases, complete, detail) = acquired;
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

    Inventory {
        sources,
        workspace_aliases,
        content_digest: hasher.finalize().to_hex().to_string(),
        metadata_complete: complete,
        metadata_detail: detail,
    }
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

    let output = match command.output() {
        Ok(output) => output,
        Err(error) => return Err(format!("could not execute cargo metadata: {error}")),
    };

    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_owned());
    }

    parse_metadata_output(&output.stdout)
}

fn parse_metadata_output(bytes: &[u8]) -> Result<Metadata, String> {
    match serde_json::from_slice(bytes) {
        Ok(metadata) => Ok(metadata),
        Err(error) => Err(format!("invalid cargo metadata JSON: {error}")),
    }
}

fn manifest_parent(manifest_path: &str) -> Result<PathBuf, String> {
    let manifest = PathBuf::from(manifest_path);
    match manifest.parent() {
        Some(parent) => Ok(parent.to_path_buf()),
        None => Err(format!(
            "manifest has no parent directory: {}",
            manifest.display()
        )),
    }
}

fn io_with_path<T>(result: std::io::Result<T>, action: &str, path: &Path) -> Result<T, String> {
    match result {
        Ok(value) => Ok(value),
        Err(error) => Err(format!("cannot {action} {}: {error}", path.display())),
    }
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

    for package in &packages {
        let package_root = manifest_parent(&package.manifest_path)?;

        if !package_root.starts_with(&metadata_root) || !package_root.starts_with(root) {
            continue;
        }
        package_roots.insert(package_root, package.name.clone());
    }

    let mut raw_targets = Vec::<(String, &'static str, PathBuf, PathBuf, Vec<Dependency>)>::new();
    for package in &packages {
        let package_root = manifest_parent(&package.manifest_path)?;

        if !package_roots.contains_key(&package_root) {
            continue;
        }

        for target in &package.targets {
            let kind = if target
                .kind
                .iter()
                .any(|kind| kind == "lib" || kind == "rlib")
            {
                "lib"
            } else if target.kind.iter().any(|kind| kind == "bin") {
                "bin"
            } else {
                continue;
            };

            let source = PathBuf::from(&target.src_path);
            if !source.starts_with(&package_root)
                || !source.starts_with(root)
                || source.extension().and_then(|value| value.to_str()) != Some("rs")
            {
                continue;
            }

            raw_targets.push((
                rust_name(&target.name),
                kind,
                source,
                package_root.to_path_buf(),
                package.dependencies.clone(),
            ));
        }
    }

    let mut name_counts = BTreeMap::<(PathBuf, String), usize>::new();
    for (import_name, _, _, package_root, _) in &raw_targets {
        *name_counts
            .entry((package_root.clone(), import_name.clone()))
            .or_default() += 1;
    }

    let mut target_roots = raw_targets
        .into_iter()
        .map(|(import_name, kind, source, package_root, dependencies)| {
            let duplicate_name = name_counts
                .get(&(package_root.clone(), import_name.clone()))
                .copied()
                .unwrap_or(0)
                > 1;
            let id = if duplicate_name {
                format!("{import_name}[{kind}]")
            } else {
                import_name.clone()
            };
            TargetRoot {
                id,
                import_name,
                kind,
                source,
                package_root,
                dependencies,
            }
        })
        .collect::<Vec<_>>();

    target_roots.sort_by(|left, right| (&left.id, &left.source).cmp(&(&right.id, &right.source)));

    let mut package_library_crates = BTreeMap::<PathBuf, (String, String)>::new();
    for target in &target_roots {
        if target.kind == "lib" {
            package_library_crates.insert(
                target.package_root.clone(),
                (target.import_name.clone(), target.id.clone()),
            );
        }
    }

    let mut aliases = WorkspaceAliases::new();
    for target in &target_roots {
        let mut crate_aliases = BTreeMap::new();
        for dependency in &target.dependencies {
            let Some(path) = &dependency.path else {
                continue;
            };
            let dependency_root = PathBuf::from(path);
            let Some((_, target_id)) = package_library_crates.get(&dependency_root) else {
                continue;
            };
            let alias = dependency.rename.as_deref().unwrap_or(&dependency.name);
            crate_aliases.insert(rust_name(alias), target_id.clone());
        }

        if target.kind != "lib" {
            if let Some((library_import_name, library_id)) =
                package_library_crates.get(&target.package_root)
            {
                crate_aliases.insert(library_import_name.clone(), library_id.clone());
                crate_aliases.insert(
                    rust_name(
                        package_roots
                            .get(&target.package_root)
                            .map(String::as_str)
                            .unwrap_or(library_import_name),
                    ),
                    library_id.clone(),
                );
            }
        }

        aliases.insert(target.id.clone(), crate_aliases);
    }

    let mut sources = Vec::new();
    let mut limitations = Vec::new();
    let mut budget = SourceBudget::default();
    collect_target_roots(
        root,
        target_roots,
        profile,
        &mut budget,
        &mut sources,
        &mut limitations,
    )?;

    limitations.sort();
    limitations.dedup();
    Ok((sources, aliases, limitations))
}

fn collect_target_roots(
    root: &Path,
    target_roots: Vec<TargetRoot>,
    profile: Option<&ProfileContext>,
    budget: &mut SourceBudget,
    sources: &mut Vec<SourceFile>,
    limitations: &mut Vec<String>,
) -> Result<(), String> {
    for target in target_roots {
        if budget.exhausted {
            break;
        }
        let mut visited = BTreeSet::new();
        collect_reachable_module(
            root,
            &target.id,
            &target.source,
            "",
            true,
            profile,
            &mut visited,
            budget,
            sources,
            limitations,
        )?;
    }
    Ok(())
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
    budget: &mut SourceBudget,
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

    if metadata.len() > MAX_SOURCE_FILE_BYTES {
        limitations.push(format!(
            "{crate_name}: module source {} exceeds the 8 MiB source limit",
            source_path.display()
        ));
        return Ok(());
    }

    let canonical = io_with_path(source_path.canonicalize(), "resolve", source_path)?;
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

    if !budget.reserve(metadata.len()) {
        limitations.push(
            "aggregate production source input exceeds the 512 MiB snapshot limit; remaining modules were not acquired"
                .into(),
        );
        return Ok(());
    }

    let bytes = io_with_path(fs::read(&canonical), "read", &canonical)?;
    let relative = canonical
        .strip_prefix(repo_root)
        .expect("canonical source path was already verified inside repository");
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
        .expect("canonical absolute source path always has a parent");
    let module_dir = if is_crate_root
        || canonical.file_name().and_then(|name| name.to_str()) == Some("mod.rs")
    {
        parent.to_path_buf()
    } else {
        let stem = canonical
            .file_stem()
            .expect("non-root module source always has a file stem");
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
        budget,
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
    budget: &mut SourceBudget,
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
                budget,
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
            budget,
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

fn fallback_detail(primary: &str, limitations: &[String]) -> Option<String> {
    match summarize_limitations(limitations) {
        Some(detail) => Some(format!("{primary}; {detail}")),
        None => Some(primary.to_owned()),
    }
}

fn fallback_inventory(root: &Path) -> Result<(Vec<SourceFile>, Vec<String>), String> {
    let mut sources = Vec::new();
    let mut limitations = Vec::new();
    let mut budget = SourceBudget::default();
    let target = root.join("target");
    collect_rust_files(
        root,
        root,
        &target,
        "unknown",
        &mut budget,
        &mut sources,
        &mut limitations,
    )?;
    limitations.sort();
    limitations.dedup();
    Ok((sources, limitations))
}

fn collect_rust_files(
    repo_root: &Path,
    directory: &Path,
    target_directory: &Path,
    crate_name: &str,
    budget: &mut SourceBudget,
    out: &mut Vec<SourceFile>,
    limitations: &mut Vec<String>,
) -> Result<(), String> {
    if budget.exhausted {
        return Ok(());
    }

    let entries = io_with_path(fs::read_dir(directory), "read", directory)?;
    let mut entries = io_with_path(
        entries.collect::<Result<Vec<_>, _>>(),
        "read directory entry",
        directory,
    )?;
    entries.sort_by_key(|entry| entry.file_name());

    for entry in entries {
        let path = entry.path();
        let file_type = io_with_path(entry.file_type(), "inspect", &path)?;

        if file_type.is_symlink() {
            limitations.push(format!(
                "fallback inventory skipped symlink {}",
                path.display()
            ));
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
            collect_rust_files(
                repo_root,
                &path,
                target_directory,
                crate_name,
                budget,
                out,
                limitations,
            )?;
            continue;
        }

        if path.extension().and_then(|value| value.to_str()) != Some("rs") {
            continue;
        }

        let metadata = io_with_path(fs::metadata(&path), "inspect", &path)?;
        if metadata.len() > MAX_SOURCE_FILE_BYTES {
            limitations.push(format!(
                "fallback inventory skipped {} because it exceeds the 8 MiB source limit",
                path.display()
            ));
            continue;
        }

        if !budget.reserve(metadata.len()) {
            limitations.push(
                "fallback source input exceeds the 512 MiB snapshot limit; remaining files were not acquired"
                    .into(),
            );
            break;
        }

        let bytes = io_with_path(fs::read(&path), "read", &path)?;
        let relative = path
            .strip_prefix(repo_root)
            .expect("fallback traversal only visits repository descendants");
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
#[path = "input_tests.rs"]
mod tests;
