use anyhow::{bail, Context, Result};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use syn::visit::Visit;
use syn::{ImplItem, Item, Visibility};
use walkdir::WalkDir;

const BEGIN_MARKER: &str = "<!-- BEGIN GENERATED SYMBOL TABLES -->";
const END_MARKER: &str = "<!-- END GENERATED SYMBOL TABLES -->";

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();

    let subcommand = args.get(1).map(|s| s.as_str());
    if subcommand != Some("manifest") {
        eprintln!("Usage: cargo xtask manifest [--check]");
        std::process::exit(1);
    }

    let check_mode = args.iter().any(|a| a == "--check");
    let workspace_root = workspace_root()?;
    let manifest_path = workspace_root.join("MANIFEST.md");

    let generated = generate_symbol_tables(&workspace_root)?;
    let updated = splice_manifest(&manifest_path, &generated)?;

    if check_mode {
        let current = fs::read_to_string(&manifest_path)
            .with_context(|| format!("Failed to read {}", manifest_path.display()))?;
        if current != updated {
            eprintln!("MANIFEST.md is out of date. Run `cargo xtask manifest` to update.");
            std::process::exit(1);
        }
        eprintln!("MANIFEST.md is up to date.");
    } else {
        fs::write(&manifest_path, &updated)
            .with_context(|| format!("Failed to write {}", manifest_path.display()))?;
        eprintln!("MANIFEST.md updated.");
    }

    Ok(())
}

fn workspace_root() -> Result<PathBuf> {
    let mut dir = std::env::current_dir()?;
    loop {
        if dir.join("Cargo.toml").exists() {
            let content = fs::read_to_string(dir.join("Cargo.toml"))?;
            if content.contains("[workspace]") {
                return Ok(dir);
            }
        }
        if !dir.pop() {
            bail!("Could not find workspace root (no Cargo.toml with [workspace] found)");
        }
    }
}

fn generate_symbol_tables(workspace_root: &Path) -> Result<String> {
    let crates_dir = workspace_root.join("crates");
    let mut crate_symbols: BTreeMap<String, Vec<SymbolEntry>> = BTreeMap::new();

    if !crates_dir.exists() {
        return Ok(String::new());
    }

    let mut crate_dirs: Vec<_> = fs::read_dir(&crates_dir)?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .collect();
    crate_dirs.sort_by_key(|e| e.file_name());

    for entry in crate_dirs {
        let crate_name = entry.file_name().to_string_lossy().to_string();
        let src_dir = entry.path().join("src");
        if !src_dir.exists() {
            continue;
        }

        let mut symbols = Vec::new();

        let mut rs_files: Vec<_> = WalkDir::new(&src_dir)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.file_type().is_file()
                    && e.path().extension().map(|ext| ext == "rs").unwrap_or(false)
            })
            .collect();
        rs_files.sort_by(|a, b| a.path().cmp(b.path()));
        for rs_entry in rs_files {
            let rel_path = rs_entry
                .path()
                .strip_prefix(workspace_root)
                .unwrap_or(rs_entry.path())
                .to_string_lossy()
                .to_string();

            let source = fs::read_to_string(rs_entry.path())
                .with_context(|| format!("Failed to read {}", rs_entry.path().display()))?;

            let file = match syn::parse_file(&source) {
                Ok(f) => f,
                Err(e) => {
                    eprintln!(
                        "Warning: failed to parse {}: {}",
                        rs_entry.path().display(),
                        e
                    );
                    continue;
                }
            };

            let mut visitor = SymbolVisitor {
                file_path: rel_path,
                symbols: Vec::new(),
            };
            visitor.visit_file(&file);
            symbols.extend(visitor.symbols);
        }

        crate_symbols.insert(crate_name, symbols);
    }

    let mut output = String::new();
    for (crate_name, symbols) in &crate_symbols {
        output.push_str(&format!("\n### {}\n\n", crate_name));
        if symbols.is_empty() {
            output.push_str("_No public symbols._\n");
        } else {
            output.push_str("| Symbol | Kind | File |\n");
            output.push_str("|:---|:---|:---|\n");
            for sym in symbols {
                output.push_str(&format!(
                    "| `{}` | {} | `{}` |\n",
                    sym.name, sym.kind, sym.file
                ));
            }
        }
    }

    Ok(output)
}

fn splice_manifest(manifest_path: &Path, generated: &str) -> Result<String> {
    let current = fs::read_to_string(manifest_path)
        .with_context(|| format!("Failed to read {}", manifest_path.display()))?;

    let begin_idx = current
        .find(BEGIN_MARKER)
        .context("Missing BEGIN GENERATED SYMBOL TABLES marker in MANIFEST.md")?;
    let end_idx = current
        .find(END_MARKER)
        .context("Missing END GENERATED SYMBOL TABLES marker in MANIFEST.md")?;

    let before = &current[..begin_idx + BEGIN_MARKER.len()];
    let after = &current[end_idx..];

    let mut result = String::new();
    result.push_str(before);
    result.push('\n');
    result.push_str(generated);
    result.push_str(after);
    // Ensure exactly one trailing newline
    if !result.ends_with('\n') {
        result.push('\n');
    }
    Ok(result)
}

struct SymbolEntry {
    name: String,
    kind: String,
    file: String,
}

struct SymbolVisitor {
    file_path: String,
    symbols: Vec<SymbolEntry>,
}

impl SymbolVisitor {
    fn is_pub(vis: &Visibility) -> bool {
        matches!(vis, Visibility::Public(_))
    }

    fn add(&mut self, name: &str, kind: &str) {
        self.symbols.push(SymbolEntry {
            name: name.to_string(),
            kind: kind.to_string(),
            file: self.file_path.clone(),
        });
    }
}

impl<'ast> Visit<'ast> for SymbolVisitor {
    fn visit_item(&mut self, item: &'ast Item) {
        match item {
            Item::Fn(f) if Self::is_pub(&f.vis) => {
                self.add(&f.sig.ident.to_string(), "fn");
            }
            Item::Struct(s) if Self::is_pub(&s.vis) => {
                self.add(&s.ident.to_string(), "struct");
            }
            Item::Enum(e) if Self::is_pub(&e.vis) => {
                self.add(&e.ident.to_string(), "enum");
            }
            Item::Trait(t) if Self::is_pub(&t.vis) => {
                self.add(&t.ident.to_string(), "trait");
            }
            Item::Type(t) if Self::is_pub(&t.vis) => {
                self.add(&t.ident.to_string(), "type");
            }
            Item::Const(c) if Self::is_pub(&c.vis) => {
                self.add(&c.ident.to_string(), "const");
            }
            Item::Static(s) if Self::is_pub(&s.vis) => {
                self.add(&s.ident.to_string(), "static");
            }
            Item::Impl(imp) => {
                // Extract pub methods from impl blocks
                let type_name = quote_type(&imp.self_ty);
                for impl_item in &imp.items {
                    if let ImplItem::Fn(method) = impl_item {
                        if Self::is_pub(&method.vis) {
                            let name = format!("{}::{}", type_name, method.sig.ident);
                            self.add(&name, "method");
                        }
                    }
                }
            }
            _ => {}
        }
        syn::visit::visit_item(self, item);
    }
}

fn quote_type(ty: &syn::Type) -> String {
    match ty {
        syn::Type::Path(p) => p
            .path
            .segments
            .iter()
            .map(|s| s.ident.to_string())
            .collect::<Vec<_>>()
            .join("::"),
        _ => "_".to_string(),
    }
}
