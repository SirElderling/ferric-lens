use std::collections::{BTreeMap, BTreeSet};

use syn::{
    visit::{self, Visit},
    BinOp, ExprBinary, ExprForLoop, ExprIf, ExprLoop, ExprMatch, ExprWhile, File, Item, ItemMod,
    ItemUse, Pat, UseTree, Visibility,
};

use crate::{
    input::SourceFile,
    model::{ImportPath, ModuleMetrics},
};

pub fn extract(source: &SourceFile) -> Result<ModuleMetrics, String> {
    let text = std::str::from_utf8(&source.bytes)
        .map_err(|error| format!("source is not valid UTF-8: {error}"))?;
    let syntax: File =
        syn::parse_file(text).map_err(|error| format!("Rust parse failed: {error}"))?;

    let mut visitor = MetricsVisitor::default();
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
        parse_complete: true,
        limitation: None,
    })
}

#[derive(Default)]
struct MetricsVisitor {
    decision_sites: usize,
    public_items: usize,
    imports: Vec<ImportPath>,
}

impl<'ast> Visit<'ast> for MetricsVisitor {
    fn visit_item(&mut self, item: &'ast Item) {
        if is_public(item) {
            self.public_items += 1;
        }
        visit::visit_item(self, item);
    }

    fn visit_item_mod(&mut self, _item: &'ast ItemMod) {
        // A child module is its own structural subject. visit_item already
        // counted the declaration when public; do not descend into its body.
    }

    fn visit_item_use(&mut self, item: &'ast ItemUse) {
        let mut prefix = Vec::new();
        flatten_use_tree(&item.tree, &mut prefix, &mut self.imports);
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

pub fn resolve_local_dependencies(modules: &mut [ModuleMetrics]) {
    let mut by_crate = BTreeMap::<String, BTreeSet<String>>::new();
    for module in modules.iter().filter(|module| module.parse_complete) {
        by_crate
            .entry(module.crate_name.clone())
            .or_default()
            .insert(module.module_path.clone());
    }

    for module in modules.iter_mut().filter(|module| module.parse_complete) {
        let Some(known) = by_crate.get(&module.crate_name) else {
            continue;
        };
        let mut dependencies = BTreeSet::new();
        for import in &module.explicit_imports {
            if let Some(target) = resolve_import(&module.module_path, &import.segments, known) {
                if target != module.module_path {
                    dependencies.insert(target);
                }
            }
        }
        module.local_dependency_modules = dependencies.into_iter().collect();
    }
}

fn resolve_import(current: &str, segments: &[String], known: &BTreeSet<String>) -> Option<String> {
    if segments.is_empty() {
        return None;
    }

    let mut base: Vec<&str> = current
        .split("::")
        .filter(|segment| !segment.is_empty())
        .collect();
    let mut index = 0;

    match segments[0].as_str() {
        "crate" => {
            base.clear();
            index = 1;
        }
        "self" => index = 1,
        "super" => {
            base.pop();
            index = 1;
            while segments.get(index).map(String::as_str) == Some("super") {
                base.pop();
                index += 1;
            }
        }
        _ => return None,
    }

    let mut candidate = base.into_iter().map(str::to_owned).collect::<Vec<_>>();
    candidate.extend(segments[index..].iter().cloned());

    while !candidate.is_empty() {
        let joined = candidate.join("::");
        if known.contains(&joined) {
            return Some(joined);
        }
        candidate.pop();
    }

    None
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::{extract, resolve_import};
    use crate::input::SourceFile;

    fn source(text: &str) -> SourceFile {
        SourceFile {
            crate_name: "demo".into(),
            module_path: "engine".into(),
            relative_path: "src/engine.rs".into(),
            bytes: text.as_bytes().to_vec(),
        }
    }

    #[test]
    fn counts_documented_decision_sites_without_child_module_body() {
        let metrics = extract(&source(
            r#"
            pub fn run(x: bool) {
                if x && true { loop { break; } }
                match x { true => (), _ => () }
            }
            mod child { fn hidden() { if true {} } }
            "#,
        ))
        .unwrap();

        assert_eq!(metrics.decision_sites, 4);
        assert_eq!(metrics.public_items, 1);
    }

    #[test]
    fn flattens_grouped_imports_deterministically() {
        let metrics = extract(&source("use crate::model::{Thing, nested::Other};")).unwrap();
        let paths = metrics
            .explicit_imports
            .iter()
            .map(|path| path.segments.join("::"))
            .collect::<Vec<_>>();
        assert_eq!(
            paths,
            vec!["crate::model::Thing", "crate::model::nested::Other"]
        );
    }

    #[test]
    fn resolves_longest_known_local_module_prefix() {
        let known = BTreeSet::from([
            "engine".to_string(),
            "model".to_string(),
            "model::nested".to_string(),
        ]);
        assert_eq!(
            resolve_import(
                "engine",
                &[
                    "crate".into(),
                    "model".into(),
                    "nested".into(),
                    "Thing".into()
                ],
                &known
            ),
            Some("model::nested".into())
        );
    }
}
