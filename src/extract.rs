use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};

use proc_macro2::Span;
use quote::ToTokens;
use syn::{
    spanned::Spanned,
    visit::{self, Visit},
    Attribute, BinOp, ExprBinary, ExprForLoop, ExprIf, ExprLoop, ExprMatch, ExprMethodCall,
    ExprWhile, File, ForeignItemFn, ImplItemFn, Item, ItemEnum, ItemFn, ItemImpl, ItemMod,
    ItemStruct, ItemTrait, ItemTraitAlias, ItemType, ItemUnion, ItemUse, Macro, Pat, TraitItemFn,
    UseTree, Visibility,
};

use crate::{
    cfg::{HostCfg, Truth},
    input::{SourceFile, WorkspaceAliases},
    model::{
        Finding, FunctionFact, FunctionKind, ImportPath, ModuleMetrics, SourceContext, TypeFact,
        TypeKind,
    },
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
    visitor.functions.sort();
    visitor.functions.dedup();
    visitor.types.sort();
    visitor.types.dedup();

    Ok(ModuleMetrics {
        crate_name: source.crate_name.clone(),
        module_path: source.module_path.clone(),
        path: source.relative_path.clone(),
        lines: text.lines().count(),
        decision_sites: visitor.decision_sites,
        public_items: visitor.public_items,
        clone_calls: visitor.clone_calls,
        functions: visitor.functions,
        types: visitor.types,
        explicit_imports: visitor.imports,
        local_dependency_modules: Vec::new(),
        structure_digest,
        parse_complete: true,
        gate_complete: visitor.gate_limitation.is_none(),
        limitation: visitor.gate_limitation,
        history: None,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct LineSpan {
    start: usize,
    end: usize,
}

#[derive(Debug, Clone)]
struct UseObservation {
    import: ImportPath,
    span: LineSpan,
}

struct MetricsVisitor<'cfg> {
    cfg: &'cfg HostCfg,
    decision_sites: usize,
    public_items: usize,
    clone_calls: usize,
    functions: Vec<FunctionFact>,
    types: Vec<TypeFact>,
    module_scope: Vec<String>,
    impl_owner: Vec<String>,
    trait_owner: Vec<(String, bool)>,
    imports: Vec<ImportPath>,
    decision_spans: Vec<LineSpan>,
    clone_spans: Vec<LineSpan>,
    use_observations: Vec<UseObservation>,
    gate_limitation: Option<String>,
}

impl<'cfg> MetricsVisitor<'cfg> {
    fn new(cfg: &'cfg HostCfg) -> Self {
        Self {
            cfg,
            decision_sites: 0,
            public_items: 0,
            clone_calls: 0,
            functions: Vec::new(),
            types: Vec::new(),
            module_scope: Vec::new(),
            impl_owner: Vec::new(),
            trait_owner: Vec::new(),
            imports: Vec::new(),
            decision_spans: Vec::new(),
            clone_spans: Vec::new(),
            use_observations: Vec::new(),
            gate_limitation: None,
        }
    }

    fn limit_gate(&mut self, reason: &str) {
        if self.gate_limitation.is_none() {
            self.gate_limitation = Some(reason.to_owned());
        }
    }

    fn scoped_name(&self, name: &str) -> String {
        if self.module_scope.is_empty() {
            name.to_owned()
        } else {
            format!("{}::{name}", self.module_scope.join("::"))
        }
    }

    fn member_enabled(&mut self, attrs: &[Attribute]) -> bool {
        if attrs.iter().any(|attr| attr.path().is_ident("cfg_attr")) {
            self.limit_gate("module contains cfg_attr syntax not yet modeled");
            return false;
        }

        match item_cfg_state(attrs, self.cfg) {
            Truth::False => false,
            Truth::Unknown => {
                self.limit_gate("module contains unresolved production cfg syntax");
                false
            }
            Truth::True => true,
        }
    }

    fn push_type(&mut self, name: &str, kind: TypeKind, visibility: &Visibility) {
        self.types.push(TypeFact {
            name: self.scoped_name(name),
            kind,
            public_declared: is_public_visibility(visibility),
        });
    }

    fn record_decision(&mut self, span: Span) {
        self.decision_sites += 1;
        self.decision_spans.push(line_span(span));
    }

    fn record_clone(&mut self, span: Span) {
        self.clone_calls += 1;
        self.clone_spans.push(line_span(span));
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
            self.module_scope.push(item.ident.to_string());
            visit::visit_item_mod(self, item);
            self.module_scope.pop();
        }
    }

    fn visit_item_fn(&mut self, item: &'ast ItemFn) {
        self.functions.push(FunctionFact {
            name: self.scoped_name(&item.sig.ident.to_string()),
            kind: FunctionKind::Function,
            public_declared: is_public_visibility(&item.vis),
        });
        visit::visit_item_fn(self, item);
    }

    fn visit_item_struct(&mut self, item: &'ast ItemStruct) {
        self.push_type(&item.ident.to_string(), TypeKind::Struct, &item.vis);
        visit::visit_item_struct(self, item);
    }

    fn visit_item_enum(&mut self, item: &'ast ItemEnum) {
        self.push_type(&item.ident.to_string(), TypeKind::Enum, &item.vis);
        visit::visit_item_enum(self, item);
    }

    fn visit_item_union(&mut self, item: &'ast ItemUnion) {
        self.push_type(&item.ident.to_string(), TypeKind::Union, &item.vis);
        visit::visit_item_union(self, item);
    }

    fn visit_item_type(&mut self, item: &'ast ItemType) {
        self.push_type(&item.ident.to_string(), TypeKind::TypeAlias, &item.vis);
        visit::visit_item_type(self, item);
    }

    fn visit_item_trait_alias(&mut self, item: &'ast ItemTraitAlias) {
        self.push_type(&item.ident.to_string(), TypeKind::TraitAlias, &item.vis);
        visit::visit_item_trait_alias(self, item);
    }

    fn visit_item_impl(&mut self, item: &'ast ItemImpl) {
        let owner = self.scoped_name(&item.self_ty.to_token_stream().to_string());
        self.impl_owner.push(owner);
        visit::visit_item_impl(self, item);
        self.impl_owner.pop();
    }

    fn visit_impl_item_fn(&mut self, item: &'ast ImplItemFn) {
        if !self.member_enabled(&item.attrs) {
            return;
        }
        let owner = self
            .impl_owner
            .last()
            .expect("impl method visited without an impl owner")
            .clone();
        self.functions.push(FunctionFact {
            name: format!("{owner}::{}", item.sig.ident),
            kind: FunctionKind::Method,
            public_declared: is_public_visibility(&item.vis),
        });
        visit::visit_impl_item_fn(self, item);
    }

    fn visit_item_trait(&mut self, item: &'ast ItemTrait) {
        self.push_type(&item.ident.to_string(), TypeKind::Trait, &item.vis);
        let owner = self.scoped_name(&item.ident.to_string());
        self.trait_owner
            .push((owner, is_public_visibility(&item.vis)));
        visit::visit_item_trait(self, item);
        self.trait_owner.pop();
    }

    fn visit_trait_item_fn(&mut self, item: &'ast TraitItemFn) {
        if !self.member_enabled(&item.attrs) {
            return;
        }
        let (owner, public_declared) = self
            .trait_owner
            .last()
            .expect("trait method visited without a trait owner")
            .clone();
        self.functions.push(FunctionFact {
            name: format!("{owner}::{}", item.sig.ident),
            kind: FunctionKind::TraitMethod,
            public_declared,
        });
        visit::visit_trait_item_fn(self, item);
    }

    fn visit_foreign_item_fn(&mut self, item: &'ast ForeignItemFn) {
        if !self.member_enabled(&item.attrs) {
            return;
        }
        self.functions.push(FunctionFact {
            name: self.scoped_name(&item.sig.ident.to_string()),
            kind: FunctionKind::ForeignFunction,
            public_declared: is_public_visibility(&item.vis),
        });
        visit::visit_foreign_item_fn(self, item);
    }

    fn visit_item_use(&mut self, item: &'ast ItemUse) {
        let first = self.imports.len();
        let mut prefix = Vec::new();
        flatten_use_tree(&item.tree, &mut prefix, &mut self.imports);
        let span = line_span(item.span());
        for import in &self.imports[first..] {
            self.use_observations.push(UseObservation {
                import: import.clone(),
                span,
            });
        }
    }

    fn visit_macro(&mut self, _node: &'ast Macro) {
        self.limit_gate("module contains macro syntax that Ferric Lens does not expand");
    }

    fn visit_expr_if(&mut self, node: &'ast ExprIf) {
        self.record_decision(node.if_token.span);
        visit::visit_expr_if(self, node);
    }

    fn visit_expr_for_loop(&mut self, node: &'ast ExprForLoop) {
        self.record_decision(node.for_token.span);
        visit::visit_expr_for_loop(self, node);
    }

    fn visit_expr_while(&mut self, node: &'ast ExprWhile) {
        self.record_decision(node.while_token.span);
        visit::visit_expr_while(self, node);
    }

    fn visit_expr_loop(&mut self, node: &'ast ExprLoop) {
        self.record_decision(node.loop_token.span);
        visit::visit_expr_loop(self, node);
    }

    fn visit_expr_match(&mut self, node: &'ast ExprMatch) {
        // Count the branch construct once rather than treating every exhaustive
        // arm as independent complexity. Large enum-to-label/state tables are
        // common Rust and otherwise dominate module-level decision counts.
        self.record_decision(node.match_token.span);
        for arm in &node.arms {
            if let Some((_, guard)) = &arm.guard {
                self.record_decision(guard.span());
            }
        }
        visit::visit_expr_match(self, node);
    }

    fn visit_expr_method_call(&mut self, node: &'ast ExprMethodCall) {
        if node.method == "clone" {
            self.record_clone(node.method.span());
        }
        visit::visit_expr_method_call(self, node);
    }

    fn visit_expr_binary(&mut self, node: &'ast ExprBinary) {
        if matches!(node.op, BinOp::And(_) | BinOp::Or(_)) {
            self.record_decision(node.op.span());
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
    is_public_visibility(visibility)
}

fn is_public_visibility(visibility: &Visibility) -> bool {
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

const MAX_CONTEXTS_PER_SIGNAL: usize = 3;
const MAX_EXCERPT_LINES: usize = 3;
const MAX_EXCERPT_CHARS: usize = 600;

struct ContextFacts {
    text: String,
    decision_spans: Vec<LineSpan>,
    clone_spans: Vec<LineSpan>,
    use_observations: Vec<UseObservation>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum SourceMetric {
    Decisions,
    Dependencies,
    Clones,
    ReverseDependents,
}

impl SourceMetric {
    fn parse(metric: &str) -> Option<Self> {
        match metric {
            "decision_sites" => Some(Self::Decisions),
            "local_dependency_modules" => Some(Self::Dependencies),
            "clone_call_syntax_sites" => Some(Self::Clones),
            "reverse_repository_dependents" => Some(Self::ReverseDependents),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Decisions => "decision_sites",
            Self::Dependencies => "local_dependency_modules",
            Self::Clones => "clone_call_syntax_sites",
            Self::ReverseDependents => "reverse_repository_dependents",
        }
    }
}

pub fn source_contexts_for_findings(
    root: &Path,
    modules: &[ModuleMetrics],
    workspace_aliases: &WorkspaceAliases,
    source_digests: &BTreeMap<String, String>,
    findings: &[Finding],
    cfg: &HostCfg,
    resolved_features_by_crate: &BTreeMap<String, Vec<String>>,
) -> Result<Vec<SourceContext>, String> {
    let requested = requested_source_contexts(findings);
    if requested.is_empty() {
        return Ok(Vec::new());
    }

    let by_subject = modules
        .iter()
        .filter(|module| module.parse_complete)
        .map(|module| (qualify(&module.crate_name, &module.module_path), module))
        .collect::<BTreeMap<_, _>>();
    let known_by_crate = known_modules_by_crate(modules);

    let mut needed_subjects = BTreeSet::new();
    for (subject, metrics) in &requested {
        if metrics
            .iter()
            .any(|metric| *metric != SourceMetric::ReverseDependents)
        {
            needed_subjects.insert(subject.clone());
        }
        if metrics.contains(&SourceMetric::ReverseDependents) {
            for module in modules.iter().filter(|module| {
                module
                    .local_dependency_modules
                    .iter()
                    .any(|dependency| dependency == subject)
            }) {
                needed_subjects.insert(qualify(&module.crate_name, &module.module_path));
            }
        }
    }

    let mut facts = BTreeMap::new();
    for subject in needed_subjects {
        let Some(module) = by_subject.get(&subject) else {
            continue;
        };
        let Some(expected_digest) = source_digests.get(&module.path) else {
            return Err(format!(
                "missing source digest while collecting finding context for {}",
                module.path
            ));
        };
        facts.insert(
            subject,
            load_context_facts(
                root,
                module,
                expected_digest,
                &resolved_features_by_crate
                    .get(&module.crate_name)
                    .map(|features| cfg.with_resolved_features(features))
                    .unwrap_or_else(|| cfg.clone()),
            )?,
        );
    }

    let mut contexts = Vec::new();
    for (subject, metrics) in requested {
        for metric in metrics {
            let metric_name = metric.as_str();
            match metric {
                SourceMetric::Decisions => {
                    if let (Some(module), Some(facts)) =
                        (by_subject.get(&subject), facts.get(&subject))
                    {
                        push_span_contexts(
                            &mut contexts,
                            &subject,
                            metric_name,
                            &module.path,
                            &facts.decision_spans,
                            &facts.text,
                        );
                    }
                }
                SourceMetric::Clones => {
                    if let (Some(module), Some(facts)) =
                        (by_subject.get(&subject), facts.get(&subject))
                    {
                        push_span_contexts(
                            &mut contexts,
                            &subject,
                            metric_name,
                            &module.path,
                            &facts.clone_spans,
                            &facts.text,
                        );
                    }
                }
                SourceMetric::Dependencies => {
                    if let (Some(module), Some(facts)) =
                        (by_subject.get(&subject), facts.get(&subject))
                    {
                        let aliases = workspace_aliases.get(&module.crate_name);
                        let spans = facts
                            .use_observations
                            .iter()
                            .filter_map(|observation| {
                                if observation.import.glob {
                                    return None;
                                }
                                match resolve_import(
                                    &module.crate_name,
                                    &module.module_path,
                                    &observation.import.segments,
                                    &known_by_crate,
                                    aliases,
                                ) {
                                    ImportResolution::Repository(target) if target != subject => {
                                        Some(observation.span)
                                    }
                                    _ => None,
                                }
                            })
                            .collect::<Vec<_>>();
                        push_span_contexts(
                            &mut contexts,
                            &subject,
                            metric_name,
                            &module.path,
                            &spans,
                            &facts.text,
                        );
                    }
                }
                SourceMetric::ReverseDependents => {
                    for module in modules.iter().filter(|module| {
                        module.parse_complete
                            && module
                                .local_dependency_modules
                                .iter()
                                .any(|dependency| dependency == &subject)
                    }) {
                        let dependent = qualify(&module.crate_name, &module.module_path);
                        let facts = facts
                            .get(&dependent)
                            .expect("reverse dependent source facts were preloaded");
                        let aliases = workspace_aliases.get(&module.crate_name);
                        let spans = facts
                            .use_observations
                            .iter()
                            .filter_map(|observation| {
                                if observation.import.glob {
                                    return None;
                                }
                                match resolve_import(
                                    &module.crate_name,
                                    &module.module_path,
                                    &observation.import.segments,
                                    &known_by_crate,
                                    aliases,
                                ) {
                                    ImportResolution::Repository(target) if target == subject => {
                                        Some(observation.span)
                                    }
                                    _ => None,
                                }
                            })
                            .collect::<Vec<_>>();
                        push_span_contexts(
                            &mut contexts,
                            &subject,
                            metric_name,
                            &module.path,
                            &spans,
                            &facts.text,
                        );
                    }
                }
            }
        }
    }

    contexts.sort();
    contexts.dedup();
    let mut counts = BTreeMap::<(String, String), usize>::new();
    contexts.retain(|context| {
        let count = counts
            .entry((context.subject.clone(), context.metric.clone()))
            .or_default();
        if *count >= MAX_CONTEXTS_PER_SIGNAL {
            return false;
        }
        *count += 1;
        true
    });
    Ok(contexts)
}

fn requested_source_contexts(findings: &[Finding]) -> BTreeMap<String, BTreeSet<SourceMetric>> {
    let mut requested = BTreeMap::<String, BTreeSet<SourceMetric>>::new();
    for finding in findings {
        for evidence in &finding.evidence {
            if let Some(metric) = SourceMetric::parse(&evidence.metric) {
                requested
                    .entry(finding.subject.clone())
                    .or_default()
                    .insert(metric);
            }
        }
    }
    requested
}

fn known_modules_by_crate(modules: &[ModuleMetrics]) -> BTreeMap<String, BTreeSet<String>> {
    let mut by_crate = BTreeMap::<String, BTreeSet<String>>::new();
    for module in modules.iter().filter(|module| module.parse_complete) {
        by_crate
            .entry(module.crate_name.clone())
            .or_default()
            .insert(module.module_path.clone());
    }
    by_crate
}

fn load_context_facts(
    root: &Path,
    module: &ModuleMetrics,
    expected_digest: &str,
    cfg: &HostCfg,
) -> Result<ContextFacts, String> {
    let path = root.join(&module.path);
    let bytes = fs::read(&path).map_err(|error| {
        format!(
            "cannot read {} for finding context: {error}",
            path.display()
        )
    })?;
    let actual_digest = blake3::hash(&bytes).to_hex().to_string();
    if actual_digest != expected_digest {
        return Err(format!(
            "source changed while collecting finding context: {}",
            module.path
        ));
    }
    let text = std::str::from_utf8(&bytes)
        .map_err(|error| format!("source is not valid UTF-8: {error}"))?
        .to_owned();
    let syntax: File =
        syn::parse_file(&text).map_err(|error| format!("Rust parse failed: {error}"))?;
    let mut visitor = MetricsVisitor::new(cfg);
    visitor.visit_file(&syntax);

    Ok(ContextFacts {
        text,
        decision_spans: visitor.decision_spans,
        clone_spans: visitor.clone_spans,
        use_observations: visitor.use_observations,
    })
}

fn push_span_contexts(
    contexts: &mut Vec<SourceContext>,
    subject: &str,
    metric: &str,
    path: &str,
    spans: &[LineSpan],
    text: &str,
) {
    let mut spans = spans.to_vec();
    spans.sort();
    spans.dedup();
    spans.sort_by(|left, right| {
        source_context_priority(metric, *right, text)
            .cmp(&source_context_priority(metric, *left, text))
            .then(left.cmp(right))
    });
    for span in spans {
        contexts.push(source_context(subject, metric, path, span, text));
    }
}

fn source_context_priority(metric: &str, span: LineSpan, text: &str) -> i32 {
    let line = text
        .lines()
        .nth(span.start.saturating_sub(1))
        .unwrap_or_default()
        .trim();

    match metric {
        "clone_call_syntax_sites" => {
            let mut score = 0;
            if line.contains("let mut ") {
                score += 80;
            }
            if line.starts_with("for ") || line.contains(" in ") {
                score += 70;
            }
            if line.contains(".name.clone()") || line.contains("label:") {
                score -= 40;
            }
            if line.contains(": self.") {
                score -= 20;
            }
            score
        }
        "decision_sites" => {
            if line.starts_with("if ") || line.contains(" if ") {
                60
            } else if line.starts_with("while ") || line.starts_with("for ") {
                50
            } else if line.contains("&&") || line.contains("||") {
                40
            } else if line.contains("=>") {
                -20
            } else {
                0
            }
        }
        _ => 0,
    }
}

fn source_context(
    subject: &str,
    metric: &str,
    path: &str,
    span: LineSpan,
    text: &str,
) -> SourceContext {
    let excerpt_end = span
        .end
        .min(span.start.saturating_add(MAX_EXCERPT_LINES - 1));
    let excerpt = text
        .lines()
        .skip(span.start.saturating_sub(1))
        .take(excerpt_end.saturating_sub(span.start) + 1)
        .collect::<Vec<_>>()
        .join("\n");
    let (excerpt, truncated_chars) = truncate_excerpt(&excerpt);

    SourceContext {
        subject: subject.to_owned(),
        metric: metric.to_owned(),
        path: path.to_owned(),
        start_line: span.start,
        end_line: span.end,
        excerpt,
        excerpt_truncated: excerpt_end < span.end || truncated_chars,
    }
}

fn truncate_excerpt(excerpt: &str) -> (String, bool) {
    if excerpt.chars().count() <= MAX_EXCERPT_CHARS {
        return (excerpt.to_owned(), false);
    }

    let mut truncated = excerpt
        .chars()
        .take(MAX_EXCERPT_CHARS.saturating_sub(1))
        .collect::<String>();
    truncated.push('…');
    (truncated, true)
}

fn line_span(span: Span) -> LineSpan {
    let start = span.start();
    let end = span.end();
    LineSpan {
        start: start.line,
        end: end.line.max(start.line),
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
        let known_current = &by_crate[&current_crate];

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
