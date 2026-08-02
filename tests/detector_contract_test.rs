use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::{bail, ensure, Context, Result};
use syn::visit::{self, Visit};
use syn::{
    Attribute, Expr, ExprCall, ExprLit, ExprMethodCall, ExprStruct, ItemImpl, ItemMod, ItemUse, Lit,
};

const DETECTOR_TRAITS: [&str; 4] = [
    "Detector",
    "WorkspaceContributor",
    "TargetBinder",
    "TargetRunner",
];

const AUDITED_READ_DIR_FILES: [&str; 5] =
    ["cargo.rs", "dart.rs", "dotnet.rs", "node.rs", "wrapper.rs"];
const AUDITED_FILE_OPEN_FILES: [&str; 2] = ["php_file.rs", "script.rs"];

#[test]
fn every_detector_hook_is_registered_exactly_once() -> Result<()> {
    let mut implementations = HookInventory::default();
    for path in detector_source_files()? {
        let syntax = parse(&path)?;
        HookImplementationVisitor(&mut implementations).visit_file(&syntax);
    }

    let registry = parse(Path::new("src/registry.rs"))?;
    let mut registrations = HookInventory::default();
    RegistryHookVisitor(&mut registrations).visit_file(&registry);

    ensure_unique_hooks("trait implementation", &implementations)?;
    ensure_unique_hooks("registry hook", &registrations)?;
    ensure!(
        implementations == registrations,
        "detector hook implementations and registry wiring differ\nimplemented: {implementations:#?}\nregistered: {registrations:#?}"
    );
    Ok(())
}

#[test]
fn detector_sources_obey_discovery_safety_contract() -> Result<()> {
    let registered_environment = dev_launcher::registry::cache_environment()
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    let mut violations = Vec::new();

    for path in detector_source_files()? {
        let syntax = parse(&path)?;
        let file_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .context("detector source file has no UTF-8 basename")?;
        let mut visitor = SafetyVisitor::new(file_name, &registered_environment);
        visitor.visit_file(&syntax);
        violations.extend(visitor.violations);
    }

    if !violations.is_empty() {
        bail!(
            "detector discovery safety contract violations:\n- {}",
            violations.join("\n- ")
        );
    }
    Ok(())
}

#[test]
fn detector_registration_metadata_is_complete() -> Result<()> {
    dev_launcher::registry::validate()?;

    for registration in dev_launcher::registry::registrations() {
        let detector = registration.id.as_str();
        ensure!(
            !registration.candidate_sources.is_empty(),
            "detector `{detector}` has no candidate source"
        );
        ensure!(
            !registration.synonyms.is_empty(),
            "detector `{detector}` has no query synonyms"
        );
        ensure!(
            !registration.conventional_roots.is_empty(),
            "detector `{detector}` has no conventional roots"
        );

        ensure_unique_values(detector, "synonym", registration.synonyms.iter().copied())?;
        ensure_unique_values(
            detector,
            "conventional root",
            registration.conventional_roots.iter().copied(),
        )?;
        ensure_unique_values(
            detector,
            "cache environment key",
            registration.cache_environment.iter().copied(),
        )?;

        for source in registration.candidate_sources {
            ensure!(
                source.metadata_priority > 0,
                "detector `{detector}` source `{}` has zero metadata priority",
                source.id
            );
            ensure!(
                !source.default_tags.is_empty(),
                "detector `{detector}` source `{}` has no default tags",
                source.id
            );
            ensure_unique_values(detector, "source tag", source.default_tags.iter().copied())?;
        }

        for tool in registration.tools {
            ensure!(
                !tool.id.as_str().trim().is_empty(),
                "detector `{detector}` has a tool with an empty id"
            );
            ensure!(
                !tool.program.trim().is_empty(),
                "detector `{detector}` tool `{}` has an empty program",
                tool.id
            );
            match tool.doctor {
                dev_launcher::registry::DoctorProbe::Command { args, timeout, .. } => {
                    ensure!(
                        !args.is_empty(),
                        "detector `{detector}` tool `{}` has an empty doctor command",
                        tool.id
                    );
                    ensure!(
                        !timeout.is_zero(),
                        "detector `{detector}` tool `{}` has a zero doctor timeout",
                        tool.id
                    );
                }
                dev_launcher::registry::DoctorProbe::PresenceOnly { reason } => ensure!(
                    !reason.trim().is_empty(),
                    "detector `{detector}` tool `{}` has an empty presence-only reason",
                    tool.id
                ),
                dev_launcher::registry::DoctorProbe::LocalMetadata(_) => {}
            }
        }
    }
    Ok(())
}

#[test]
fn safety_audit_rejects_forbidden_detector_behavior() -> Result<()> {
    let syntax = syn::parse_file(
        r#"
        fn detect() {
            std::process::Command::new("tool").status();
            std::fs::write("generated", "contents");
            let _ = std::env::var("UNREGISTERED_KEY");
        }

        #[cfg(test)]
        mod tests {
            fn fixture() { std::fs::write("fixture", "contents"); }
        }
        "#,
    )?;
    let registered_environment = BTreeSet::new();
    let mut visitor = SafetyVisitor::new("example.rs", &registered_environment);
    visitor.visit_file(&syntax);

    ensure!(
        visitor.violations.len() == 4,
        "expected four production violations and no test-only violation: {:#?}",
        visitor.violations
    );
    Ok(())
}

#[derive(Debug, Default, Eq, PartialEq)]
struct HookInventory(BTreeMap<String, BTreeMap<String, usize>>);

impl HookInventory {
    fn record(&mut self, role: &str, implementation: &str) {
        *self
            .0
            .entry(role.to_owned())
            .or_default()
            .entry(implementation.to_owned())
            .or_default() += 1;
    }
}

struct HookImplementationVisitor<'a>(&'a mut HookInventory);

impl<'ast> Visit<'ast> for HookImplementationVisitor<'_> {
    fn visit_item_impl(&mut self, node: &'ast ItemImpl) {
        let Some((_, trait_path, _)) = &node.trait_ else {
            return visit::visit_item_impl(self, node);
        };
        let Some(trait_name) = trait_path
            .segments
            .last()
            .map(|segment| segment.ident.to_string())
        else {
            return visit::visit_item_impl(self, node);
        };
        if !DETECTOR_TRAITS.contains(&trait_name.as_str()) {
            return visit::visit_item_impl(self, node);
        }
        let syn::Type::Path(self_type) = node.self_ty.as_ref() else {
            return visit::visit_item_impl(self, node);
        };
        if let Some(implementation) = self_type.path.segments.last() {
            self.0
                .record(&trait_name, &implementation.ident.to_string());
        }
        visit::visit_item_impl(self, node);
    }

    fn visit_item_mod(&mut self, node: &'ast ItemMod) {
        if !is_cfg_test(&node.attrs) {
            visit::visit_item_mod(self, node);
        }
    }
}

struct RegistryHookVisitor<'a>(&'a mut HookInventory);

impl<'ast> Visit<'ast> for RegistryHookVisitor<'_> {
    fn visit_expr_struct(&mut self, node: &'ast ExprStruct) {
        if node
            .path
            .segments
            .last()
            .is_some_and(|segment| segment.ident == "DetectorRegistration")
        {
            for field in &node.fields {
                let Some(role) = member_name(&field.member)
                    .as_deref()
                    .and_then(registry_trait_for_field)
                else {
                    continue;
                };
                for implementation in referenced_types(&field.expr) {
                    self.0.record(role, &implementation);
                }
            }
        }
        visit::visit_expr_struct(self, node);
    }

    fn visit_item_mod(&mut self, node: &'ast ItemMod) {
        if !is_cfg_test(&node.attrs) {
            visit::visit_item_mod(self, node);
        }
    }
}

struct SafetyVisitor<'a> {
    file_name: &'a str,
    registered_environment: &'a BTreeSet<&'static str>,
    violations: Vec<String>,
}

impl<'a> SafetyVisitor<'a> {
    fn new(file_name: &'a str, registered_environment: &'a BTreeSet<&'static str>) -> Self {
        Self {
            file_name,
            registered_environment,
            violations: Vec::new(),
        }
    }

    fn reject(&mut self, detail: impl std::fmt::Display) {
        self.violations
            .push(format!("{}: {detail}", self.file_name));
    }

    fn inspect_call(&mut self, call: &ExprCall) {
        let Some(path) = expression_path(&call.func) else {
            return;
        };
        let segments = path.split("::").collect::<Vec<_>>();

        if forbidden_call(&segments) {
            self.reject(format!(
                "`{path}` is forbidden during discovery; detectors must not spawn, access the network, or write files"
            ));
        }

        if path_ends_with(&segments, &["fs", "read_dir"])
            && !AUDITED_READ_DIR_FILES.contains(&self.file_name)
        {
            self.reject(format!(
                "`{path}` bypasses the shared bounded index; use `DiscoveryFiles`/`FileIndex`, or document and audit a bounded direct probe"
            ));
        }

        if path_ends_with(&segments, &["File", "open"])
            && !AUDITED_FILE_OPEN_FILES.contains(&self.file_name)
        {
            self.reject(format!(
                "`{path}` is not an audited bounded read; use `DiscoveryFiles`, or add a reviewed fixed-size probe"
            ));
        }

        if is_environment_read(&segments) {
            let Some(first) = call.args.first() else {
                self.reject(format!("`{path}` has no environment-key argument"));
                return;
            };
            match literal_string(first) {
                Some(key) if !self.registered_environment.contains(key.as_str()) => {
                    self.reject(format!(
                        "environment key `{key}` is read but absent from detector cache metadata"
                    ));
                }
                Some(_) => {}
                None if self.file_name != "wrapper.rs" => self.reject(format!(
                    "dynamic environment read `{path}` cannot be checked against cache metadata"
                )),
                None => {}
            }
        }
    }
}

impl<'ast> Visit<'ast> for SafetyVisitor<'_> {
    fn visit_item_mod(&mut self, node: &'ast ItemMod) {
        if !is_cfg_test(&node.attrs) {
            visit::visit_item_mod(self, node);
        }
    }

    fn visit_item_use(&mut self, node: &'ast ItemUse) {
        let mut imports = Vec::new();
        flatten_use_tree(Vec::new(), &node.tree, &mut imports);
        for import in imports {
            let parts = import.split("::").collect::<Vec<_>>();
            if forbidden_import(&parts) {
                self.reject(format!("forbidden discovery import `{import}`"));
            }
        }
        visit::visit_item_use(self, node);
    }

    fn visit_expr_call(&mut self, node: &'ast ExprCall) {
        self.inspect_call(node);
        visit::visit_expr_call(self, node);
    }

    fn visit_expr_method_call(&mut self, node: &'ast ExprMethodCall) {
        if matches!(
            node.method.to_string().as_str(),
            "output" | "spawn" | "status"
        ) {
            self.reject(format!(
                "method `{}` is forbidden because detector discovery must not spawn subprocesses",
                node.method
            ));
        }
        visit::visit_expr_method_call(self, node);
    }

    fn visit_expr_lit(&mut self, node: &'ast ExprLit) {
        if self.file_name == "wrapper.rs" {
            if let Lit::Str(value) = &node.lit {
                let key = value.value();
                if looks_like_environment_key(&key)
                    && !self.registered_environment.contains(key.as_str())
                {
                    self.reject(format!(
                        "environment-like wrapper key `{key}` is absent from detector cache metadata"
                    ));
                }
            }
        }
        visit::visit_expr_lit(self, node);
    }
}

fn detector_source_files() -> Result<Vec<PathBuf>> {
    let mut paths = std::fs::read_dir("src/detect")
        .context("read src/detect")?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().is_some_and(|extension| extension == "rs"))
        .collect::<Vec<_>>();
    paths.sort();
    Ok(paths)
}

fn parse(path: &Path) -> Result<syn::File> {
    let source = std::fs::read_to_string(path)
        .with_context(|| format!("read detector source {}", path.display()))?;
    syn::parse_file(&source).with_context(|| format!("parse detector source {}", path.display()))
}

fn is_cfg_test(attributes: &[Attribute]) -> bool {
    attributes.iter().any(|attribute| {
        attribute.path().is_ident("cfg")
            && matches!(
                &attribute.meta,
                syn::Meta::List(list) if list.tokens.to_string() == "test"
            )
    })
}

fn member_name(member: &syn::Member) -> Option<String> {
    match member {
        syn::Member::Named(ident) => Some(ident.to_string()),
        syn::Member::Unnamed(_) => None,
    }
}

fn registry_trait_for_field(field: &str) -> Option<&'static str> {
    match field {
        "detector" => Some("Detector"),
        "workspace" => Some("WorkspaceContributor"),
        "target_binders" => Some("TargetBinder"),
        "target_runners" => Some("TargetRunner"),
        _ => None,
    }
}

fn referenced_types(expression: &Expr) -> Vec<String> {
    let mut names = Vec::new();
    collect_referenced_types(expression, &mut names);
    names
}

fn collect_referenced_types(expression: &Expr, names: &mut Vec<String>) {
    match expression {
        Expr::Reference(reference) => {
            if let Expr::Path(path) = reference.expr.as_ref() {
                if let Some(segment) = path.path.segments.last() {
                    names.push(segment.ident.to_string());
                }
            } else {
                collect_referenced_types(&reference.expr, names);
            }
        }
        Expr::Array(array) => {
            for element in &array.elems {
                collect_referenced_types(element, names);
            }
        }
        Expr::Call(call) => {
            for argument in &call.args {
                collect_referenced_types(argument, names);
            }
        }
        Expr::Group(group) => collect_referenced_types(&group.expr, names),
        Expr::Paren(paren) => collect_referenced_types(&paren.expr, names),
        Expr::Tuple(tuple) => {
            for element in &tuple.elems {
                collect_referenced_types(element, names);
            }
        }
        _ => {}
    }
}

fn ensure_unique_hooks(label: &str, inventory: &HookInventory) -> Result<()> {
    for (role, hooks) in &inventory.0 {
        for (implementation, count) in hooks {
            ensure!(
                *count == 1,
                "{label} `{implementation}` for `{role}` occurs {count} times"
            );
        }
    }
    Ok(())
}

fn ensure_unique_values<'a>(
    detector: &str,
    kind: &str,
    values: impl IntoIterator<Item = &'a str>,
) -> Result<()> {
    let mut seen = BTreeSet::new();
    for value in values {
        ensure!(
            !value.trim().is_empty(),
            "detector `{detector}` has an empty {kind}"
        );
        ensure!(
            seen.insert(value),
            "detector `{detector}` repeats {kind} `{value}`"
        );
    }
    Ok(())
}

fn expression_path(expression: &Expr) -> Option<String> {
    let Expr::Path(path) = expression else {
        return None;
    };
    Some(
        path.path
            .segments
            .iter()
            .map(|segment| segment.ident.to_string())
            .collect::<Vec<_>>()
            .join("::"),
    )
}

fn literal_string(expression: &Expr) -> Option<String> {
    let Expr::Lit(literal) = expression else {
        return None;
    };
    let Lit::Str(value) = &literal.lit else {
        return None;
    };
    Some(value.value())
}

fn is_environment_read(path: &[&str]) -> bool {
    path_ends_with(path, &["env", "var"]) || path_ends_with(path, &["env", "var_os"])
}

fn looks_like_environment_key(value: &str) -> bool {
    !value.is_empty()
        && value.contains('_')
        && value
            .bytes()
            .all(|byte| byte == b'_' || byte.is_ascii_uppercase() || byte.is_ascii_digit())
}

fn forbidden_call(path: &[&str]) -> bool {
    path_ends_with(path, &["Command", "new"])
        || path_ends_with(path, &["process", "exit"])
        || path_ends_with(path, &["fs", "write"])
        || path_ends_with(path, &["fs", "copy"])
        || path_ends_with(path, &["fs", "create_dir"])
        || path_ends_with(path, &["fs", "create_dir_all"])
        || path_ends_with(path, &["fs", "remove_dir"])
        || path_ends_with(path, &["fs", "remove_dir_all"])
        || path_ends_with(path, &["fs", "remove_file"])
        || path_ends_with(path, &["fs", "rename"])
        || path_ends_with(path, &["fs", "set_permissions"])
        || path_ends_with(path, &["File", "create"])
        || path_ends_with(path, &["OpenOptions", "new"])
        || path_ends_with(path, &["fs", "read"])
        || path_ends_with(path, &["fs", "read_to_string"])
        || has_network_prefix(path)
        || has_walker(path)
}

fn forbidden_import(path: &[&str]) -> bool {
    path_ends_with(path, &["std", "process"])
        || path_starts_with(path, &["std", "process"])
        || path_ends_with(path, &["std", "net"])
        || path_starts_with(path, &["std", "net"])
        || path_ends_with(path, &["std", "env", "var"])
        || path_ends_with(path, &["std", "env", "var_os"])
        || forbidden_fs_import(path)
        || has_network_prefix(path)
        || has_walker(path)
}

fn forbidden_fs_import(path: &[&str]) -> bool {
    const FORBIDDEN: [&str; 12] = [
        "copy",
        "create_dir",
        "create_dir_all",
        "read",
        "read_to_string",
        "remove_dir",
        "remove_dir_all",
        "remove_file",
        "rename",
        "set_permissions",
        "write",
        "OpenOptions",
    ];
    path_starts_with(path, &["std", "fs"])
        && path.last().is_some_and(|name| FORBIDDEN.contains(name))
}

fn has_network_prefix(path: &[&str]) -> bool {
    path_starts_with(path, &["std", "net"])
        || path
            .first()
            .is_some_and(|root| matches!(*root, "hyper" | "reqwest" | "ureq"))
        || path_starts_with(path, &["tokio", "net"])
        || path_starts_with(path, &["tokio", "process"])
}

fn has_walker(path: &[&str]) -> bool {
    path.first().is_some_and(|root| *root == "walkdir")
        || path
            .iter()
            .any(|segment| matches!(*segment, "WalkBuilder" | "WalkDir"))
}

fn path_starts_with(path: &[&str], expected: &[&str]) -> bool {
    path.get(..expected.len()) == Some(expected)
}

fn path_ends_with(path: &[&str], expected: &[&str]) -> bool {
    path.get(path.len().saturating_sub(expected.len())..) == Some(expected)
}

fn flatten_use_tree(prefix: Vec<String>, tree: &syn::UseTree, output: &mut Vec<String>) {
    match tree {
        syn::UseTree::Path(path) => {
            let mut next = prefix;
            next.push(path.ident.to_string());
            flatten_use_tree(next, &path.tree, output);
        }
        syn::UseTree::Name(name) => {
            let mut path = prefix;
            path.push(name.ident.to_string());
            output.push(path.join("::"));
        }
        syn::UseTree::Rename(rename) => {
            let mut path = prefix;
            path.push(rename.ident.to_string());
            output.push(path.join("::"));
        }
        syn::UseTree::Glob(_) => output.push(prefix.join("::")),
        syn::UseTree::Group(group) => {
            for item in &group.items {
                flatten_use_tree(prefix.clone(), item, output);
            }
        }
    }
}
