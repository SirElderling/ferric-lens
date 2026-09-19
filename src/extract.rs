use std::collections::{BTreeMap, BTreeSet};

use quote::ToTokens;
use syn::{
    visit::{self, Visit},
    Attribute, BinOp, ExprBinary, ExprForLoop, ExprIf, ExprLoop, ExprMatch, ExprWhile, File, Item,
    ItemMod, ItemUse, Macro, Pat, UseTree, Visibility,
};

use crate::{
    cfg::{HostCfg, Truth},
    input::{SourceFile, WorkspaceAliases},
    model::{ImportPath, ModuleMetrics},
};

pub fn extract(source: &SourceFile, cfg: &HostCfg) -> Result<ModuleMetrics, String> {
    let text = std::str::from_utf8(&source.bytes)
        .map_err(|error| format!("source is not valid UTF-8: {error}"))?;
    let syntax: File =
        syn::parse_file(text).map_err(|error| format!("Rust parse failed: {error}"))?;

    let structure_digest = blake3::hash(syntax.to_token_stream().to_string().as_bytes())
        .to_hex()
        .to_string();

    let mut visitor = MetricsVisitor::new(cfg);
    visitor.visit_file(&syntax);
    visitor
        .imports
        .sort_by(|a, b| a.segments.cmp(&b.segments).then(a.glob.cmp(&b.glob)));
    visitor.imports.dedup();

    Ok(ModuleMetrics {
        crate_name: source.crate_name.clone(),
        module_path: source.module_path.clone(),
        path: source.relative_path.clone(),
        lines: text.lines().count(),
        decision_sites: visitor.decision_sites,
        public_items: visitor.public_items,
        explicit_imports: visitor.imports,
        local_dependency_modules: Vec::new(),
        structure_digest,
        parse_complete: true,
        gate_complete: visitor.gate_limitation.is_none(),
        limitation: visitor.gate_limitation,
        history: None,
    })
}

struct MetricsVisitor<'cfg> {
    cfg: &'cfg HostCfg,
    decision_sites: usize,
    public_items: usize,
    imports: Vec<ImportPath>,
    gate_limitation: Option<String>,
}

impl<'cfg> MetricsVisitor<'cfg> {
    fn new(cfg: &'cfg HostCfg) -> Self {
        Self {
            cfg,
            decision_sites: 0,
            public_items: 0,
            imports: Vec::new(),
            gate_limitation: None,
        }
    }

    fn limit_gate(&mut self, reason: &str) {
        if self.gate_limitation.is_none() {
            self.gate_limitation = Some(reason.to_owned());
        }
    }
}

impl<'ast> Visit<'ast> for MetricsVisitor<'_> {
    fn visit_item(&mut self, item: &'ast Item) {
        let attrs = item_attributes(item);
        if attrs.iter().any(|attr| attr.path().is_ident("cfg_attr")) {
            self.limit_gate("module contains cfg_attr syntax not yet modeled");
            return;
        }

        match item_cfg_state(attrs, self.cfg) {
            Truth::False => return,
            Truth::Unknown => {
                self.limit_gate("module contains unresolved production cfg syntax");
                return;
            }
            Truth::True => {}
        }

        if is_public(item) {
            self.public_items += 1;
        }
        visit::visit_item(self, item);
    }

    fn visit_item_mod(&mut self, item: &'ast ItemMod) {
        // File-backed child modules are separate structural subjects discovered
        // by input inventory. Inline modules have no independent file subject in
        // V1, so retain their facts in the containing file rather than dropping
        // executable code from analysis.
        if item.content.is_some() {
            visit::visit_item_mod(self, item);
        }
    }

    fn visit_item_use(&mut self, item: &'ast ItemUse) {
        let mut prefix = Vec::new();
        flatten_use_tree(&item.tree, &mut prefix, &mut self.imports);
    }

    fn visit_macro(&mut self, _node: &'ast Macro) {
        self.limit_gate("module contains macro syntax that Ferric Lens does not expand");
    }

    fn visit_expr_if(&mut self, node: &'ast ExprIf) {
        self.decision_sites += 1;
        visit::visit_expr_if(self, node);
    }

    fn visit_expr_for_loop(&mut self, node: &'ast ExprForLoop) {
        self.decision_sites += 1;
        visit::visit_expr_for_loop(self, node);
    }

    fn visit_expr_while(&mut self, node: &'ast ExprWhile) {
        self.decision_sites += 1;
        visit::visit_expr_while(self, node);
    }

    fn visit_expr_loop(&mut self, node: &'ast ExprLoop) {
        self.decision_sites += 1;
        visit::visit_expr_loop(self, node);
    }

    fn visit_expr_match(&mut self, node: &'ast ExprMatch) {
        for arm in &node.arms {
            if !matches!(&arm.pat, Pat::Wild(_)) {
                self.decision_sites += 1;
            }
            if arm.guard.is_some() {
                self.decision_sites += 1;
            }
        }
        visit::visit_expr_match(self, node);
    }

    fn visit_expr_binary(&mut self, node: &'ast ExprBinary) {
        if matches!(node.op, BinOp::And(_) | BinOp::Or(_)) {
            self.decision_sites += 1;
        }
        visit::visit_expr_binary(self, node);
    }
}

fn item_attributes(item: &Item) -> &[Attribute] {
    match item {
        Item::Const(item) => &item.attrs,
        Item::Enum(item) => &item.attrs,
        Item::ExternCrate(item) => &item.attrs,
        Item::Fn(item) => &item.attrs,
        Item::ForeignMod(item) => &item.attrs,
        Item::Impl(item) => &item.attrs,
        Item::Macro(item) => &item.attrs,
        Item::Mod(item) => &item.attrs,
        Item::Static(item) => &item.attrs,
        Item::Struct(item) => &item.attrs,
        Item::Trait(item) => &item.attrs,
        Item::TraitAlias(item) => &item.attrs,
        Item::Type(item) => &item.attrs,
        Item::Union(item) => &item.attrs,
        Item::Use(item) => &item.attrs,
        _ => &[],
    }
}

fn item_cfg_state(attrs: &[Attribute], cfg: &HostCfg) -> Truth {
    let mut state = Truth::True;

    for attr in attrs.iter().filter(|attr| attr.path().is_ident("cfg")) {
        let Ok(meta) = attr.parse_args::<syn::Meta>() else {
            return Truth::Unknown;
        };
        state = combine_and(state, cfg.evaluate(&meta));
        if state == Truth::False {
            return Truth::False;
        }
    }

    state
}

fn combine_and(left: Truth, right: Truth) -> Truth {
    match (left, right) {
        (Truth::False, _) | (_, Truth::False) => Truth::False,
        (Truth::Unknown, _) | (_, Truth::Unknown) => Truth::Unknown,
        (Truth::True, Truth::True) => Truth::True,
    }
}

fn is_public(item: &Item) -> bool {
    let visibility = match item {
        Item::Const(item) => &item.vis,
        Item::Enum(item) => &item.vis,
        Item::ExternCrate(item) => &item.vis,
        Item::Fn(item) => &item.vis,
        Item::ForeignMod(_) => return false,
        Item::Impl(_) => return false,
        Item::Macro(_) => return false,
        Item::Mod(item) => &item.vis,
        Item::Static(item) => &item.vis,
        Item::Struct(item) => &item.vis,
        Item::Trait(item) => &item.vis,
        Item::TraitAlias(item) => &item.vis,
        Item::Type(item) => &item.vis,
        Item::Union(item) => &item.vis,
        Item::Use(item) => &item.vis,
        _ => return false,
    };
    matches!(visibility, Visibility::Public(_))
}

fn flatten_use_tree(tree: &UseTree, prefix: &mut Vec<String>, out: &mut Vec<ImportPath>) {
    match tree {
        UseTree::Path(path) => {
            prefix.push(path.ident.to_string());
            flatten_use_tree(&path.tree, prefix, out);
            prefix.pop();
        }
        UseTree::Name(name) => {
            let mut segments = prefix.clone();
            segments.push(name.ident.to_string());
            out.push(ImportPath {
                segments,
                glob: false,
            });
        }
        UseTree::Rename(rename) => {
            let mut segments = prefix.clone();
            segments.push(rename.ident.to_string());
            out.push(ImportPath {
                segments,
                glob: false,
            });
        }
        UseTree::Glob(_) => out.push(ImportPath {
            segments: prefix.clone(),
            glob: true,
        }),
        UseTree::Group(group) => {
            for item in &group.items {
                flatten_use_tree(item, prefix, out);
            }
        }
    }
}

pub fn resolve_workspace_dependencies(
    modules: &mut [ModuleMetrics],
    workspace_aliases: &WorkspaceAliases,
) {
    let mut by_crate = BTreeMap::<String, BTreeSet<String>>::new();
    for module in modules.iter().filter(|module| module.parse_complete) {
        by_crate
            .entry(module.crate_name.clone())
            .or_default()
            .insert(module.module_path.clone());
    }

    for module in modules.iter_mut().filter(|module| module.parse_complete) {
        let current_crate = module.crate_name.clone();
        let current_module = module.module_path.clone();
        let Some(known_current) = by_crate.get(&current_crate) else {
            continue;
        };

        let aliases = workspace_aliases.get(&current_crate);
        let mut dependencies = BTreeSet::new();
        let mut unresolved_repository_import = false;
        let mut glob_import = false;

        for import in &module.explicit_imports {
            if import.glob {
                glob_import = true;
                continue;
            }

            match resolve_import(
                &current_crate,
                &current_module,
                &import.segments,
                &by_crate,
                aliases,
            ) {
                ImportResolution::Repository(target) => {
                    let current_subject = qualify(&current_crate, &current_module);
                    if target != current_subject {
                        dependencies.insert(target);
                    }
                }
                ImportResolution::UnresolvedRepository => {
                    unresolved_repository_import = true;
                }
                ImportResolution::External => {}
            }
        }

        if glob_import {
            module.gate_complete = false;
            append_limitation(
                module,
                "module contains a glob import whose gate dependency surface is ambiguous",
            );
        }

        if unresolved_repository_import {
            module.gate_complete = false;
            append_limitation(
                module,
                "one or more repository-owned explicit imports could not be resolved",
            );
        }

        // Keep the historical field name in the v1 JSON schema. Its values are
        // repository-qualified modules, including cross-crate dependencies.
        module.local_dependency_modules = dependencies.into_iter().collect();

        debug_assert!(known_current.contains(&current_module));
    }
}

enum ImportResolution {
    Repository(String),
    External,
    UnresolvedRepository,
}

fn resolve_import(
    current_crate: &str,
    current_module: &str,
    segments: &[String],
    known_by_crate: &BTreeMap<String, BTreeSet<String>>,
    aliases: Option<&BTreeMap<String, String>>,
) -> ImportResolution {
    if segments.is_empty() {
        return ImportResolution::External;
    }

    let Some(known_current) = known_by_crate.get(current_crate) else {
        return ImportResolution::UnresolvedRepository;
    };

    match segments[0].as_str() {
        "crate" | "self" | "super" => {
            return resolve_in_crate(current_module, segments, known_current)
                .map(|module| ImportResolution::Repository(qualify(current_crate, &module)))
                .unwrap_or(ImportResolution::UnresolvedRepository);
        }
        _ => {}
    }

    if let Some(target_crate) = aliases.and_then(|aliases| aliases.get(&segments[0])) {
        let Some(known_target) = known_by_crate.get(target_crate) else {
            return ImportResolution::UnresolvedRepository;
        };
        return resolve_absolute(&segments[1..], known_target)
            .map(|module| ImportResolution::Repository(qualify(target_crate, &module)))
            .unwrap_or(ImportResolution::UnresolvedRepository);
    }

    let first = segments[0].as_str();
    let looks_local = known_current.iter().any(|module| {
        module
            .split("::")
            .next()
            .is_some_and(|segment| segment == first)
    });
    if looks_local {
        return resolve_absolute(segments, known_current)
            .map(|module| ImportResolution::Repository(qualify(current_crate, &module)))
            .unwrap_or(ImportResolution::UnresolvedRepository);
    }

    ImportResolution::External
}

fn resolve_in_crate(
    current: &str,
    segments: &[String],
    known: &BTreeSet<String>,
) -> Option<String> {
    let mut base = current
        .split("::")
        .filter(|segment| !segment.is_empty())
        .map(str::to_owned)
        .collect::<Vec<_>>();

    let index = match segments.first().map(String::as_str) {
        Some("crate") => {
            base.clear();
            1
        }
        Some("self") => 1,
        Some("super") => {
            base.pop();
            let mut index = 1;
            while segments.get(index).map(String::as_str) == Some("super") {
                base.pop();
                index += 1;
            }
            index
        }
        _ => return None,
    };

    base.extend(segments[index..].iter().cloned());
    longest_module_prefix(base, known)
}

fn resolve_absolute(segments: &[String], known: &BTreeSet<String>) -> Option<String> {
    longest_module_prefix(segments.to_vec(), known)
}

fn longest_module_prefix(mut candidate: Vec<String>, known: &BTreeSet<String>) -> Option<String> {
    loop {
        let joined = candidate.join("::");
        if known.contains(&joined) {
            return Some(joined);
        }
        if candidate.is_empty() {
            return None;
        }
        candidate.pop();
    }
}

fn qualify(crate_name: &str, module_path: &str) -> String {
    if module_path.is_empty() {
        crate_name.to_owned()
    } else {
        format!("{crate_name}::{module_path}")
    }
}

fn append_limitation(module: &mut ModuleMetrics, reason: &str) {
    match &mut module.limitation {
        Some(existing) if !existing.contains(reason) => {
            existing.push_str("; ");
            existing.push_str(reason);
        }
        Some(_) => {}
        None => module.limitation = Some(reason.to_owned()),
    }
}

#[cfg(test)]
#[path = "extract_tests.rs"]
mod tests;
