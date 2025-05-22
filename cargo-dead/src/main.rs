use cargo_metadata::{Dependency, DependencyKind, MetadataCommand, Package};
use std::{collections::HashSet, fs, path::Path};
use toml_edit::{Document, Item};
use walkdir::WalkDir;
use syn::visit::Visit;
use clap::{Parser, Subcommand, Args};

#[derive(Parser, Debug)]
#[command(name = "cargo-dead", about = "Detect and optionally remove unused dependencies in a Rust project or workspace.")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    Check(FilterOptions),
    Fix(FilterOptions),
}

#[derive(Args, Debug)]
struct FilterOptions {
    #[arg(long)]
    only_dev: bool,
    #[arg(long)]
    only_build: bool,
    #[arg(long)]
    only_regular: bool,
}

struct CrateVisitor {
    used_crates: HashSet<String>,
}

impl<'ast> Visit<'ast> for CrateVisitor {
    fn visit_path(&mut self, path: &'ast syn::Path) {
        if let Some(first_segment) = path.segments.first() {
            self.used_crates.insert(first_segment.ident.to_string());
        }
        syn::visit::visit_path(self, path);
    }
}

fn scan_rust_files(dir: &Path) -> anyhow::Result<HashSet<String>> {
    let mut visitor = CrateVisitor { used_crates: HashSet::new() };
    for entry in WalkDir::new(dir).into_iter().filter_map(Result::ok) {
        let path = entry.path();
        if path.extension().map_or(false, |ext| ext == "rs") {
            let content = fs::read_to_string(path)?;
            if let Ok(syntax) = syn::parse_file(&content) {
                visitor.visit_file(&syntax);
            }
        }
    }
    Ok(visitor.used_crates)
}

fn get_dependency_names(dependencies: &[Dependency], kind: DependencyKind) -> HashSet<String> {
    dependencies.iter().filter(|dep| dep.kind == kind).map(|dep| dep.name.clone()).collect()
}

fn analyze_package(package: &Package, fix: bool, filter: &FilterOptions) -> anyhow::Result<()> {
    println!("\nAnalyzing package: {}", package.name);

    let declared_normal = get_dependency_names(&package.dependencies, DependencyKind::Normal);
    let declared_dev = get_dependency_names(&package.dependencies, DependencyKind::Development);
    let declared_build = get_dependency_names(&package.dependencies, DependencyKind::Build);

    let package_root = package.manifest_path.parent().unwrap().as_std_path();
    let mut used_crates = HashSet::new();

    for dir in ["src", "tests"] {
        let dir_path = package_root.join(dir);
        if dir_path.exists() {
            used_crates.extend(scan_rust_files(&dir_path)?);
        }
    }

    let build_rs = package_root.join("build.rs");
    if build_rs.exists() {
        let content = fs::read_to_string(&build_rs)?;
        if let Ok(syntax) = syn::parse_file(&content) {
            let mut visitor = CrateVisitor { used_crates: HashSet::new() };
            visitor.visit_file(&syntax);
            used_crates.extend(visitor.used_crates);
        }
    }

    let cargo_toml_path = package_root.join("Cargo.toml");
    let mut doc: Document = fs::read_to_string(&cargo_toml_path)?.parse()?;
    let mut changed = false;

    let check_normal = filter.only_regular || (!filter.only_dev && !filter.only_build);
    let check_dev = filter.only_dev || (!filter.only_regular && !filter.only_build);
    let check_build = filter.only_build || (!filter.only_regular && !filter.only_dev);

    if check_normal {
        for dep in &declared_normal {
            if !used_crates.contains(dep) {
                println!("Unused dependency: {}", dep);
                if fix {
                    if let Item::Table(ref mut tbl) = doc["dependencies"] {
                        tbl.remove(dep);
                        changed = true;
                    }
                }
            }
        }
    }

    if check_dev {
        for dep in &declared_dev {
            if !used_crates.contains(dep) {
                println!("Unused dev-dependency: {}", dep);
                if fix {
                    if let Item::Table(ref mut tbl) = doc["dev-dependencies"] {
                        tbl.remove(dep);
                        changed = true;
                    }
                }
            }
        }
    }

    if check_build {
        for dep in &declared_build {
            if !used_crates.contains(dep) {
                println!("Unused build-dependency: {}", dep);
                if fix {
                    if let Item::Table(ref mut tbl) = doc["build-dependencies"] {
                        tbl.remove(dep);
                        changed = true;
                    }
                }
            }
        }
    }

    if fix && changed {
        fs::write(&cargo_toml_path, doc.to_string())?;
        println!("Updated {}", cargo_toml_path.display());
    }

    Ok(())
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let metadata = MetadataCommand::new().exec()?;

    match cli.command {
        Commands::Check(ref filter) => {
            for package in &metadata.packages {
                if metadata.workspace_members.contains(&package.id) {
                    analyze_package(package, false, filter)?;
                }
            }
        }
        Commands::Fix(ref filter) => {
            for package in &metadata.packages {
                if metadata.workspace_members.contains(&package.id) {
                    analyze_package(package, true, filter)?;
                }
            }
        }
    }

    Ok(())
}
