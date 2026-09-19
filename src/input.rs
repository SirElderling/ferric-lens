use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use serde::Deserialize;
use syn::{Attribute, Item, ItemMod};

use crate::{
    cfg::{HostCfg, Truth},
    profile::ProfileContext,
};

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

#[derive(Debug, Clone)]
struct CargoInputEntry {
    path: PathBuf,
    label: String,
    bytes: Option<Vec<u8>>,
}

#[derive(Debug, Clone)]
struct CargoInputSnapshot {
    entries: Vec<CargoInputEntry>,
    digest: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AuxiliaryTargetSummary {
    pub tests: usize,
    pub benches: usize,
    pub examples: usize,
    pub build_scripts: usize,
    pub proc_macros: usize,
}

#[derive(Debug)]
pub struct Inventory {
    pub sources: Vec<SourceFile>,
    pub workspace_aliases: WorkspaceAliases,
    pub resolved_features_by_crate: BTreeMap<String, Vec<String>>,
    pub content_digest: String,
    pub metadata_complete: bool,
    pub metadata_detail: Option<String>,
    pub cargo_resolution_digest: Option<String>,
    pub cargo_input_digest: String,
    pub auxiliary_targets: AuxiliaryTargetSummary,
}

#[derive(Debug, Deserialize)]
struct Metadata {
    packages: Vec<Package>,
    workspace_members: Vec<String>,
    workspace_root: String,
    #[serde(default)]
    resolve: Option<Resolve>,
}

#[derive(Debug, Deserialize)]
struct Resolve {
    #[serde(default)]
    nodes: Vec<ResolveNode>,
}

#[derive(Debug, Deserialize)]
struct ResolveNode {
    id: String,
    #[serde(default)]
    features: Vec<String>,
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
    resolved_features: Option<Vec<String>>,
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
    let cargo_inputs = cargo_input_snapshot(&root, metadata.as_ref().ok())?;
    assemble_inventory(&root, metadata, profile, &cargo_inputs)
}

fn assemble_inventory(
    root: &Path,
    metadata: Result<Metadata, String>,
    profile: Option<&ProfileContext>,
    cargo_inputs: &CargoInputSnapshot,
) -> Result<Inventory, String> {
    let auxiliary_targets = metadata
        .as_ref()
        .map(auxiliary_target_summary)
        .unwrap_or_default();
    let cargo_resolution_digest = match metadata.as_ref() {
        Ok(metadata) => Some(cargo_resolution_identity_with_digest(
            root,
            metadata,
            &cargo_inputs.digest,
        )?),
        Err(_) => None,
    };

    let acquired = acquire_inventory(root, metadata, profile)?;
    finalize_verified_inventory(
        root,
        acquired,
        cargo_inputs,
        cargo_resolution_digest,
        auxiliary_targets,
    )
}

fn finalize_verified_inventory(
    root: &Path,
    acquired: AcquiredInventory,
    cargo_inputs: &CargoInputSnapshot,
    cargo_resolution_digest: Option<String>,
    auxiliary_targets: AuxiliaryTargetSummary,
) -> Result<Inventory, String> {
    verify_stable_inputs_snapshot(root, &acquired.0, cargo_inputs)?;
    let mut inventory = finalize_inventory(acquired);
    let mut hasher = blake3::Hasher::new();
    hasher.update(inventory.content_digest.as_bytes());
    hasher.update(cargo_inputs.digest.as_bytes());
    if let Some(resolution) = &cargo_resolution_digest {
        hasher.update(resolution.as_bytes());
    }
    inventory.content_digest = hasher.finalize().to_hex().to_string();
    inventory.cargo_resolution_digest = cargo_resolution_digest;
    inventory.cargo_input_digest = cargo_inputs.digest.clone();
    inventory.auxiliary_targets = auxiliary_targets;
    Ok(inventory)
}

type AcquiredInventory = (
    Vec<SourceFile>,
    WorkspaceAliases,
    BTreeMap<String, Vec<String>>,
    bool,
    Option<String>,
);
type MetadataInventory = (
    Vec<SourceFile>,
    WorkspaceAliases,
    BTreeMap<String, Vec<String>>,
    Vec<String>,
);

fn acquire_inventory(
    root: &Path,
    metadata: Result<Metadata, String>,
    profile: Option<&ProfileContext>,
) -> Result<AcquiredInventory, String> {
    match metadata {
        Ok(metadata) => match inventory_from_metadata(root, metadata, profile) {
            Ok((sources, aliases, resolved_features_by_crate, limitations)) => Ok((
                sources,
                aliases,
                resolved_features_by_crate,
                limitations.is_empty(),
                summarize_limitations(&limitations),
            )),
            Err(error) => {
                let (sources, limitations) = fallback_inventory(root)?;
                Ok((
                    sources,
                    WorkspaceAliases::new(),
                    BTreeMap::new(),
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
                BTreeMap::new(),
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
    let (mut sources, workspace_aliases, resolved_features_by_crate, complete, detail) = acquired;
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
        resolved_features_by_crate,
        content_digest: hasher.finalize().to_hex().to_string(),
        metadata_complete: complete,
        metadata_detail: detail,
        cargo_resolution_digest: None,
        cargo_input_digest: String::new(),
        auxiliary_targets: AuxiliaryTargetSummary::default(),
    }
}
fn auxiliary_target_summary(metadata: &Metadata) -> AuxiliaryTargetSummary {
    let members = metadata
        .workspace_members
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let mut summary = AuxiliaryTargetSummary::default();

    for package in metadata
        .packages
        .iter()
        .filter(|package| members.contains(package.id.as_str()))
    {
        for target in &package.targets {
            if target.kind.iter().any(|kind| kind == "test") {
                summary.tests += 1;
            }
            if target.kind.iter().any(|kind| kind == "bench") {
                summary.benches += 1;
            }
            if target.kind.iter().any(|kind| kind == "example") {
                summary.examples += 1;
            }
            if target.kind.iter().any(|kind| kind == "custom-build") {
                summary.build_scripts += 1;
            }
            if target.kind.iter().any(|kind| kind == "proc-macro") {
                summary.proc_macros += 1;
            }
        }
    }

    summary
}

fn cargo_input_snapshot(
    root: &Path,
    metadata: Option<&Metadata>,
) -> Result<CargoInputSnapshot, String> {
    let mut paths = BTreeSet::from([root.join("Cargo.toml"), root.join("Cargo.lock")]);
    if let Some(metadata) = metadata {
        let members = metadata
            .workspace_members
            .iter()
            .map(String::as_str)
            .collect::<BTreeSet<_>>();
        for package in metadata
            .packages
            .iter()
            .filter(|package| members.contains(package.id.as_str()))
        {
            let manifest = PathBuf::from(&package.manifest_path);
            if manifest.starts_with(root) {
                paths.insert(manifest);
            }
        }
    }

    let mut entries = Vec::new();
    let mut hasher = blake3::Hasher::new();
    for path in paths {
        let label = path
            .strip_prefix(root)
            .map(slash_path)
            .unwrap_or_else(|_| path.to_string_lossy().into_owned());
        let bytes = match fs::read(&path) {
            Ok(bytes) => Some(bytes),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => {
                return Err(format!(
                    "cannot read Cargo input {}: {error}",
                    path.display()
                ));
            }
        };
        hasher.update(label.as_bytes());
        hasher.update(&[0]);
        match &bytes {
            Some(bytes) => {
                hasher.update(&[1]);
                hasher.update(bytes);
            }
            None => {
                hasher.update(&[0]);
            }
        }
        hasher.update(&[0xff]);
        entries.push(CargoInputEntry { path, label, bytes });
    }

    Ok(CargoInputSnapshot {
        entries,
        digest: hasher.finalize().to_hex().to_string(),
    })
}

#[cfg(test)]
fn cargo_input_digest(root: &Path) -> Result<String, String> {
    Ok(cargo_input_snapshot(root, None)?.digest)
}

fn verify_cargo_inputs(expected: &CargoInputSnapshot) -> Result<(), String> {
    for entry in &expected.entries {
        let actual = match fs::read(&entry.path) {
            Ok(bytes) => Some(bytes),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => {
                return Err(format!(
                    "cannot re-read Cargo input {}: {error}",
                    entry.path.display()
                ));
            }
        };
        if actual != entry.bytes {
            return Err(format!(
                "Cargo input {} changed during analysis",
                entry.label
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
fn cargo_resolution_identity(root: &Path, metadata: &Metadata) -> Result<String, String> {
    let cargo_inputs = cargo_input_snapshot(root, Some(metadata))?;
    cargo_resolution_identity_with_digest(root, metadata, &cargo_inputs.digest)
}

fn cargo_resolution_identity_with_digest(
    root: &Path,
    metadata: &Metadata,
    cargo_input_digest: &str,
) -> Result<String, String> {
    let members = metadata
        .workspace_members
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let resolved_features = resolved_features_by_package(metadata);
    let mut records = Vec::new();

    for package in metadata
        .packages
        .iter()
        .filter(|package| members.contains(package.id.as_str()))
    {
        let manifest = PathBuf::from(&package.manifest_path);
        let Ok(relative_manifest) = manifest.strip_prefix(root) else {
            continue;
        };
        let manifest_bytes = fs::read(&manifest).map_err(|error| {
            format!("cannot read Cargo manifest {}: {error}", manifest.display())
        })?;
        let mut targets = package
            .targets
            .iter()
            .filter_map(|target| {
                let source = PathBuf::from(&target.src_path);
                let relative = source.strip_prefix(root).ok()?;
                let mut kinds = target.kind.clone();
                kinds.sort();
                Some(format!(
                    "{}|{}|{}",
                    target.name,
                    kinds.join(","),
                    slash_path(relative)
                ))
            })
            .collect::<Vec<_>>();
        targets.sort();

        let mut dependencies = package
            .dependencies
            .iter()
            .map(|dependency| {
                let path = dependency.path.as_deref().map_or("", |path| {
                    Path::new(path)
                        .strip_prefix(root)
                        .ok()
                        .and_then(Path::to_str)
                        .unwrap_or("<outside-repository>")
                });
                format!(
                    "{}|{}|{}",
                    dependency.name,
                    dependency.rename.as_deref().unwrap_or(""),
                    path
                )
            })
            .collect::<Vec<_>>();
        dependencies.sort();

        records.push((
            package.name.clone(),
            slash_path(relative_manifest),
            manifest_bytes,
            targets,
            dependencies,
            resolved_features
                .get(&package.id)
                .cloned()
                .unwrap_or_default(),
        ));
    }
    records.sort_by(|left, right| (&left.0, &left.1).cmp(&(&right.0, &right.1)));

    let mut hasher = blake3::Hasher::new();
    hasher.update(cargo_input_digest.as_bytes());
    for (name, manifest, bytes, targets, dependencies, features) in records {
        hasher.update(name.as_bytes());
        hasher.update(&[0]);
        hasher.update(manifest.as_bytes());
        hasher.update(&[0]);
        hasher.update(&bytes);
        hasher.update(&[0]);
        for target in targets {
            hasher.update(target.as_bytes());
            hasher.update(&[0xfe]);
        }
        for dependency in dependencies {
            hasher.update(dependency.as_bytes());
            hasher.update(&[0xfd]);
        }
        for feature in features {
            hasher.update(feature.as_bytes());
            hasher.update(&[0xfc]);
        }
    }
    Ok(hasher.finalize().to_hex().to_string())
}

fn verify_stable_inputs_snapshot(
    root: &Path,
    sources: &[SourceFile],
    expected_cargo_inputs: &CargoInputSnapshot,
) -> Result<(), String> {
    for source in sources {
        let path = root.join(&source.relative_path);
        let bytes = fs::read(&path).map_err(|_| {
            format!(
                "repository source {} changed during analysis",
                source.relative_path
            )
        })?;
        if bytes != source.bytes {
            return Err(format!(
                "repository source {} changed during analysis",
                source.relative_path
            ));
        }
    }

    verify_cargo_inputs(expected_cargo_inputs)?;
    Ok(())
}

#[cfg(test)]
pub(crate) fn verify_stable_inputs(
    root: &Path,
    sources: &[SourceFile],
    expected_cargo_digest: &str,
) -> Result<(), String> {
    for source in sources {
        let path = root.join(&source.relative_path);
        let bytes = fs::read(&path).map_err(|_| {
            format!(
                "repository source {} changed during analysis",
                source.relative_path
            )
        })?;
        if bytes != source.bytes {
            return Err(format!(
                "repository source {} changed during analysis",
                source.relative_path
            ));
        }
    }
    if cargo_input_digest(root)? != expected_cargo_digest {
        return Err("Cargo inputs changed during analysis".into());
    }
    Ok(())
}

fn load_metadata(root: &Path, profile: Option<&ProfileContext>) -> Result<Metadata, String> {
    let mut command = Command::new("cargo");
    command
        .current_dir(root)
        .args(["metadata", "--format-version", "1", "--offline", "--locked"]);

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

fn resolved_features_by_package(metadata: &Metadata) -> BTreeMap<String, Vec<String>> {
    let mut by_package = BTreeMap::new();
    let Some(resolve) = &metadata.resolve else {
        return by_package;
    };
    for node in &resolve.nodes {
        let mut features = node.features.clone();
        features.sort();
        features.dedup();
        by_package.insert(node.id.clone(), features);
    }
    by_package
}

fn inventory_from_metadata(
    root: &Path,
    metadata: Metadata,
    profile: Option<&ProfileContext>,
) -> Result<MetadataInventory, String> {
    let metadata_root = PathBuf::from(&metadata.workspace_root);
    let members: BTreeSet<&str> = metadata
        .workspace_members
        .iter()
        .map(String::as_str)
        .collect();
    let resolved_features_by_package = resolved_features_by_package(&metadata);

    let packages = metadata
        .packages
        .into_iter()
        .filter(|package| members.contains(package.id.as_str()))
        .collect::<Vec<_>>();

    let mut package_roots = BTreeMap::<PathBuf, String>::new();
    let mut package_root_by_id = BTreeMap::<String, PathBuf>::new();

    for package in &packages {
        let package_root = manifest_parent(&package.manifest_path)?;

        if !package_root.starts_with(&metadata_root) || !package_root.starts_with(root) {
            continue;
        }
        package_roots.insert(package_root.clone(), package.name.clone());
        package_root_by_id.insert(package.id.clone(), package_root);
    }

    let mut raw_targets = Vec::<(
        String,
        &'static str,
        PathBuf,
        PathBuf,
        Vec<Dependency>,
        Option<Vec<String>>,
    )>::new();
    for package in &packages {
        let Some(package_root) = package_root_by_id.get(&package.id).cloned() else {
            continue;
        };

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
                resolved_features_by_package.get(&package.id).cloned(),
            ));
        }
    }

    let mut name_counts = BTreeMap::<(PathBuf, String), usize>::new();
    for (import_name, _, _, package_root, _, _) in &raw_targets {
        *name_counts
            .entry((package_root.clone(), import_name.clone()))
            .or_default() += 1;
    }

    let mut target_roots = raw_targets
        .into_iter()
        .map(
            |(import_name, kind, source, package_root, dependencies, resolved_features)| {
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
                    resolved_features,
                }
            },
        )
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

    let resolved_features_by_crate = target_roots
        .iter()
        .filter_map(|target| {
            target
                .resolved_features
                .as_ref()
                .map(|features| (target.id.clone(), features.clone()))
        })
        .collect::<BTreeMap<_, _>>();

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
    Ok((sources, aliases, resolved_features_by_crate, limitations))
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
        let target_cfg = profile.map(|profile| {
            target
                .resolved_features
                .as_ref()
                .map(|features| profile.cfg.with_resolved_features(features))
                .unwrap_or_else(|| profile.cfg.clone())
        });
        collect_reachable_module(
            root,
            &target.id,
            &target.source,
            "",
            true,
            target_cfg.as_ref(),
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
    cfg: Option<&HostCfg>,
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

    collect_canonical_module(
        repo_root,
        crate_name,
        source_path,
        module_path,
        is_crate_root,
        cfg,
        metadata.len(),
        source_path.canonicalize(),
        visited,
        budget,
        out,
        limitations,
    )
}

#[allow(clippy::too_many_arguments)]
fn collect_canonical_module(
    repo_root: &Path,
    crate_name: &str,
    source_path: &Path,
    module_path: &str,
    is_crate_root: bool,
    cfg: Option<&HostCfg>,
    source_len: u64,
    canonical_result: std::io::Result<PathBuf>,
    visited: &mut BTreeSet<PathBuf>,
    budget: &mut SourceBudget,
    out: &mut Vec<SourceFile>,
    limitations: &mut Vec<String>,
) -> Result<(), String> {
    let canonical = io_with_path(canonical_result, "resolve", source_path)?;
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

    if !budget.reserve(source_len) {
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
        cfg,
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
    cfg: Option<&HostCfg>,
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

        match module_cfg_state(&module.attrs, cfg) {
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
                cfg,
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
            cfg,
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

fn module_cfg_state(attrs: &[Attribute], cfg: Option<&HostCfg>) -> Truth {
    let Some(cfg) = cfg else {
        return Truth::True;
    };

    let mut state = Truth::True;
    for attr in attrs.iter().filter(|attr| attr.path().is_ident("cfg")) {
        let Ok(meta) = attr.parse_args::<syn::Meta>() else {
            return Truth::Unknown;
        };
        state = match (state, cfg.evaluate(&meta)) {
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

fn collect_directory_entries<I>(entries: I, directory: &Path) -> Result<Vec<fs::DirEntry>, String>
where
    I: IntoIterator<Item = std::io::Result<fs::DirEntry>>,
{
    entries
        .into_iter()
        .map(|entry| io_with_path(entry, "read directory entry", directory))
        .collect()
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

    let paths = read_directory_paths(directory)?;
    collect_rust_paths(
        repo_root,
        paths,
        target_directory,
        crate_name,
        budget,
        out,
        limitations,
    )
}

fn read_directory_paths(directory: &Path) -> Result<Vec<PathBuf>, String> {
    let entries = io_with_path(fs::read_dir(directory), "read", directory)?;
    directory_paths_from_entries(collect_directory_entries(entries, directory))
}

fn directory_paths_from_entries(
    entries: Result<Vec<fs::DirEntry>, String>,
) -> Result<Vec<PathBuf>, String> {
    let mut entries = entries?;
    entries.sort_by_key(|entry| entry.file_name());
    Ok(entries.into_iter().map(|entry| entry.path()).collect())
}

fn collect_rust_paths(
    repo_root: &Path,
    paths: Vec<PathBuf>,
    target_directory: &Path,
    crate_name: &str,
    budget: &mut SourceBudget,
    out: &mut Vec<SourceFile>,
    limitations: &mut Vec<String>,
) -> Result<(), String> {
    for path in paths {
        let metadata = io_with_path(fs::symlink_metadata(&path), "inspect", &path)?;
        let file_type = metadata.file_type();

        if file_type.is_symlink() {
            limitations.push(format!(
                "fallback inventory skipped symlink {}",
                path.display()
            ));
            continue;
        }

        if file_type.is_dir() {
            let name = path.file_name().unwrap_or_default().to_string_lossy();
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
