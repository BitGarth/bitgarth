use std::env;
use std::fs;
use std::path::{Path, PathBuf};

fn generate_migrations(
    migrations_rel_dir: &str,
    output_filename: &str,
    out_dir: &Path,
) -> Result<(), String> {
    let manifest_dir =
        env::var("CARGO_MANIFEST_DIR").map_err(|e| format!("CARGO_MANIFEST_DIR not set: {e}"))?;
    let migrations_path = Path::new(&manifest_dir).join(migrations_rel_dir);

    println!("cargo:rerun-if-changed={}", migrations_path.display());

    let read_dir = fs::read_dir(&migrations_path)
        .map_err(|e| format!("Failed to read {}: {e}", migrations_path.display()))?;

    let mut entries: Vec<_> = read_dir
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().map(|x| x == "sql").unwrap_or(false))
        .collect();
    entries.sort_by_key(|e| e.file_name());

    let mut pairs = Vec::new();
    for entry in &entries {
        let path = entry.path();
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .ok_or_else(|| format!("Invalid migration filename: {}", path.display()))?;

        println!("cargo:rerun-if-changed={}", path.display());

        pairs.push(format!("    ({stem:?}, include_str!({path:?})),\n"));
    }

    let content = format!("&[\n{}]", pairs.join(""));
    let out_file = out_dir.join(output_filename);
    fs::write(&out_file, content)
        .map_err(|e| format!("Failed to write {}: {e}", out_file.display()))?;

    Ok(())
}

fn get_git_short_sha() -> Option<String> {
    let output = std::process::Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()?;
    if output.status.success() {
        let sha = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !sha.is_empty() {
            return Some(sha);
        }
    }
    None
}

/// Build-arg fallback for environments where `.git` is absent (Docker/fly).
/// Reads `GIT_SHORT_SHA`, trims it, and rejects empty values.
fn git_short_sha_from_env() -> Option<String> {
    let sha = env::var("GIT_SHORT_SHA").ok()?;
    let sha = sha.trim().to_string();
    if sha.is_empty() { None } else { Some(sha) }
}

fn main() {
    println!("cargo:rustc-check-cfg=cfg(bitgarth_db_unit_only)");

    let out_dir = match env::var("OUT_DIR") {
        Ok(v) => PathBuf::from(v),
        Err(e) => {
            eprintln!("build error: OUT_DIR not set: {e}");
            std::process::exit(1);
        }
    };

    for (dir, filename) in &[
        ("migrations/app", "app_migrations.rs"),
        ("migrations/user", "user_migrations.rs"),
    ] {
        if let Err(e) = generate_migrations(dir, filename, &out_dir) {
            eprintln!("build error: {e}");
            std::process::exit(1);
        }
    }

    // Set git short SHA for version string. Prefer the live git command;
    // fall back to a GIT_SHORT_SHA build-arg (Docker/fly have no .git).
    if let Some(sha) = get_git_short_sha().or_else(git_short_sha_from_env) {
        println!("cargo:rustc-env=GIT_SHORT_SHA={}", sha);
    }
    println!("cargo:rerun-if-env-changed=GIT_SHORT_SHA");
    // Rerun if git HEAD changes (for version string)
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/index");
}
