#[cfg(all(test, not(bitgarth_db_unit_only)))]
mod tests {
    use std::collections::BTreeSet;
    use std::fs;
    use std::path::{Path, PathBuf};

    use syn::visit::{self, Visit};
    use syn::{Attribute, ImplItem, Item, Path as SynPath, TraitItem, UseTree};

    const DISALLOWED_REQWEST_PATHS: &[&[&str]] = &[
        &["reqwest", "Client"],
        &["reqwest", "ClientBuilder"],
        &["reqwest", "blocking", "Client"],
        &["reqwest", "blocking", "ClientBuilder"],
    ];

    #[derive(Default)]
    struct ForbiddenPathVisitor {
        violations: BTreeSet<String>,
    }

    impl<'ast> Visit<'ast> for ForbiddenPathVisitor {
        fn visit_item(&mut self, node: &'ast Item) {
            if has_cfg_test(item_attrs(node)) {
                return;
            }
            visit::visit_item(self, node);
        }

        fn visit_impl_item(&mut self, node: &'ast ImplItem) {
            if has_cfg_test(impl_item_attrs(node)) {
                return;
            }
            visit::visit_impl_item(self, node);
        }

        fn visit_trait_item(&mut self, node: &'ast TraitItem) {
            if has_cfg_test(trait_item_attrs(node)) {
                return;
            }
            visit::visit_trait_item(self, node);
        }

        fn visit_item_use(&mut self, node: &'ast syn::ItemUse) {
            if has_cfg_test(&node.attrs) {
                return;
            }

            let mut collected = Vec::new();
            let mut prefix = Vec::new();
            collect_use_tree_paths(&node.tree, &mut prefix, &mut collected);
            for segments in collected {
                if matches_disallowed_segments(&segments) {
                    self.violations.insert(segments.join("::"));
                }
            }

            visit::visit_item_use(self, node);
        }

        fn visit_path(&mut self, node: &'ast SynPath) {
            if matches_disallowed_path(node) {
                self.violations.insert(path_to_string(node));
            }
            visit::visit_path(self, node);
        }
    }

    #[test]
    fn enforce_traced_http_clients_outside_traces_module() {
        let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        let files = collect_workspace_rust_files(manifest_dir)
            .unwrap_or_else(|err| panic!("failed to collect workspace Rust files: {err}"));

        let mut violations = Vec::new();
        for file_path in files {
            let relative_path = file_path
                .strip_prefix(manifest_dir)
                .unwrap_or(file_path.as_path());
            if is_allowed_traces_path(relative_path) {
                continue;
            }

            let source = fs::read_to_string(&file_path)
                .unwrap_or_else(|err| panic!("failed to read {}: {err}", relative_path.display()));
            match scan_source_for_disallowed_paths(&source) {
                Ok(file_violations) => {
                    for violation in file_violations {
                        violations.push(format!("{}: {}", relative_path.display(), violation));
                    }
                }
                Err(err) => violations.push(format!(
                    "{}: parse error while scanning for reqwest policy: {err}",
                    relative_path.display()
                )),
            }
        }

        violations.sort();
        assert!(
            violations.is_empty(),
            "Raw reqwest client usage is only allowed under src/traces/ and in crates/bitgarth-cli/src/client.rs.\nOffending references/imports:\n{}",
            violations.join("\n")
        );
    }

    #[test]
    fn scan_source_reports_disallowed_paths() {
        let source = r#"
            use reqwest::blocking::Client;
            fn build() {
                let _ = reqwest::Client::new();
            }
        "#;

        let violations =
            scan_source_for_disallowed_paths(source).expect("source parsing should succeed");

        assert!(
            violations.iter().any(|v| v == "reqwest::blocking::Client"),
            "expected blocking client import violation, got: {violations:?}"
        );
        assert!(
            violations.iter().any(|v| v.starts_with("reqwest::Client")),
            "expected async client reference violation, got: {violations:?}"
        );
    }

    #[test]
    fn scan_source_ignores_cfg_test_subtree() {
        let source = r#"
            #[cfg(test)]
            mod tests {
                use reqwest::ClientBuilder;
                fn helper() {
                    let _ = reqwest::blocking::Client::builder();
                }
            }
        "#;

        let violations =
            scan_source_for_disallowed_paths(source).expect("source parsing should succeed");

        assert!(
            violations.is_empty(),
            "expected cfg(test) subtree to be ignored, got: {violations:?}"
        );
    }

    #[test]
    fn scan_source_does_not_ignore_cfg_not_test_items() {
        let source = r#"
            #[cfg(not(test))]
            fn build() {
                let _ = reqwest::ClientBuilder::new();
            }
        "#;

        let violations =
            scan_source_for_disallowed_paths(source).expect("source parsing should succeed");

        assert!(
            violations
                .iter()
                .any(|v| v.starts_with("reqwest::ClientBuilder")),
            "expected cfg(not(test)) item to be scanned, got: {violations:?}"
        );
    }

    fn scan_source_for_disallowed_paths(source: &str) -> Result<Vec<String>, syn::Error> {
        let file = syn::parse_file(source)?;
        let mut visitor = ForbiddenPathVisitor::default();
        visitor.visit_file(&file);
        Ok(visitor.violations.into_iter().collect())
    }

    fn collect_rust_files_recursive(dir: &Path) -> Result<Vec<PathBuf>, String> {
        let mut entries = fs::read_dir(dir)
            .map_err(|err| format!("failed to read directory {}: {err}", dir.display()))?
            .map(|entry| entry.map(|dir_entry| dir_entry.path()))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|err| format!("failed to enumerate {}: {err}", dir.display()))?;

        entries.sort();

        let mut files = Vec::new();
        for entry in entries {
            if entry.is_dir() {
                files.extend(collect_rust_files_recursive(&entry)?);
            } else if entry.extension().is_some_and(|ext| ext == "rs") {
                files.push(entry);
            }
        }
        Ok(files)
    }

    fn collect_workspace_rust_files(manifest_dir: &Path) -> Result<Vec<PathBuf>, String> {
        let mut files = collect_rust_files_recursive(&manifest_dir.join("src"))?;
        let crates_dir = manifest_dir.join("crates");
        if !crates_dir.exists() {
            return Ok(files);
        }

        let mut members = fs::read_dir(&crates_dir)
            .map_err(|err| format!("failed to read directory {}: {err}", crates_dir.display()))?
            .map(|entry| entry.map(|dir_entry| dir_entry.path()))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|err| format!("failed to enumerate {}: {err}", crates_dir.display()))?;
        members.sort();
        for member in members {
            let src_dir = member.join("src");
            if member.join("Cargo.toml").is_file() && src_dir.is_dir() {
                files.extend(collect_rust_files_recursive(&src_dir)?);
            }
        }
        Ok(files)
    }

    fn is_allowed_traces_path(path: &Path) -> bool {
        path.starts_with(Path::new("src").join("traces"))
            || path == Path::new("crates/bitgarth-cli/src/client.rs")
    }

    #[test]
    fn reqwest_exception_is_exactly_the_cli_client_file() {
        assert!(is_allowed_traces_path(Path::new(
            "crates/bitgarth-cli/src/client.rs"
        )));
        assert!(!is_allowed_traces_path(Path::new(
            "crates/bitgarth-cli/src/other.rs"
        )));
    }

    fn matches_disallowed_path(path: &SynPath) -> bool {
        let segments = path
            .segments
            .iter()
            .map(|segment| segment.ident.to_string())
            .collect::<Vec<_>>();
        matches_disallowed_segments(&segments)
    }

    fn matches_disallowed_segments(segments: &[String]) -> bool {
        DISALLOWED_REQWEST_PATHS.iter().any(|disallowed| {
            segments.len() >= disallowed.len()
                && segments
                    .iter()
                    .zip(disallowed.iter())
                    .all(|(actual, expected)| actual == expected)
        })
    }

    fn path_to_string(path: &SynPath) -> String {
        path.segments
            .iter()
            .map(|segment| segment.ident.to_string())
            .collect::<Vec<_>>()
            .join("::")
    }

    fn collect_use_tree_paths(
        tree: &UseTree,
        prefix: &mut Vec<String>,
        output: &mut Vec<Vec<String>>,
    ) {
        match tree {
            UseTree::Path(path) => {
                prefix.push(path.ident.to_string());
                collect_use_tree_paths(&path.tree, prefix, output);
                prefix.pop();
            }
            UseTree::Name(name) => {
                let mut full = prefix.clone();
                full.push(name.ident.to_string());
                output.push(full);
            }
            UseTree::Rename(rename) => {
                let mut full = prefix.clone();
                full.push(rename.ident.to_string());
                output.push(full);
            }
            UseTree::Group(group) => {
                for item in &group.items {
                    collect_use_tree_paths(item, prefix, output);
                }
            }
            UseTree::Glob(_) => {}
        }
    }

    fn has_cfg_test(attrs: &[Attribute]) -> bool {
        attrs.iter().any(is_cfg_test_attribute)
    }

    fn is_cfg_test_attribute(attr: &Attribute) -> bool {
        if !attr.path().is_ident("cfg") {
            return false;
        }

        let syn::Meta::List(cfg_list) = &attr.meta else {
            return false;
        };

        matches!(
            syn::parse2::<syn::Meta>(cfg_list.tokens.clone()),
            Ok(syn::Meta::Path(path)) if path.is_ident("test")
        )
    }

    fn item_attrs(item: &Item) -> &[Attribute] {
        const EMPTY: &[Attribute] = &[];
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
            _ => EMPTY,
        }
    }

    fn impl_item_attrs(item: &ImplItem) -> &[Attribute] {
        const EMPTY: &[Attribute] = &[];
        match item {
            ImplItem::Const(item) => &item.attrs,
            ImplItem::Fn(item) => &item.attrs,
            ImplItem::Macro(item) => &item.attrs,
            ImplItem::Type(item) => &item.attrs,
            _ => EMPTY,
        }
    }

    fn trait_item_attrs(item: &TraitItem) -> &[Attribute] {
        const EMPTY: &[Attribute] = &[];
        match item {
            TraitItem::Const(item) => &item.attrs,
            TraitItem::Fn(item) => &item.attrs,
            TraitItem::Macro(item) => &item.attrs,
            TraitItem::Type(item) => &item.attrs,
            _ => EMPTY,
        }
    }
}
