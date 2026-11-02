use crate::error::{AppError, Result};
use serde_json::{Value, json};
use std::{fs, path::Path};

/// The tracked Worker project, file by file. The template directory is also a
/// working npm project, so embedding it wholesale would ship `node_modules`,
/// build outputs, or local `.dev.vars` secrets inside afmail.
pub(crate) const PROJECT: [(&str, &str); 12] = [
    (
        ".gitignore",
        include_str!("../../../templates/cloudflare-worker/.gitignore"),
    ),
    (
        "README.md",
        include_str!("../../../templates/cloudflare-worker/README.md"),
    ),
    (
        "cloudflare.config.ts",
        include_str!("../../../templates/cloudflare-worker/cloudflare.config.ts"),
    ),
    (
        "package-lock.json",
        include_str!("../../../templates/cloudflare-worker/package-lock.json"),
    ),
    (
        "package.json",
        include_str!("../../../templates/cloudflare-worker/package.json"),
    ),
    (
        "src/index.ts",
        include_str!("../../../templates/cloudflare-worker/src/index.ts"),
    ),
    (
        "src/policy.ts",
        include_str!("../../../templates/cloudflare-worker/src/policy.ts"),
    ),
    (
        "src/quota.ts",
        include_str!("../../../templates/cloudflare-worker/src/quota.ts"),
    ),
    (
        "test/index.spec.ts",
        include_str!("../../../templates/cloudflare-worker/test/index.spec.ts"),
    ),
    (
        "tsconfig.json",
        include_str!("../../../templates/cloudflare-worker/tsconfig.json"),
    ),
    (
        "vitest.config.ts",
        include_str!("../../../templates/cloudflare-worker/vitest.config.ts"),
    ),
    (
        "wrangler.jsonc",
        include_str!("../../../templates/cloudflare-worker/wrangler.jsonc"),
    ),
];
pub(crate) fn scaffold(path: &Path) -> Result<Value> {
    if path.exists()
        && (!path.is_dir()
            || fs::read_dir(path)
                .map_err(|error| AppError::io("read scaffold directory", &error))?
                .next()
                .is_some())
    {
        return Err(AppError::new(
            "path_not_empty",
            "Worker scaffold directory must be absent or empty",
        ));
    }
    fs::create_dir_all(path).map_err(|error| AppError::io("create scaffold directory", &error))?;
    for (name, source) in PROJECT {
        let target = path.join(name);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| AppError::io("create scaffold directory", &error))?;
        }
        let contents = source
            .replace("__WORKER_NAME__", "afmail-inbound")
            .replace("__BUCKET_NAME__", "afmail-spool");
        fs::write(&target, contents)
            .map_err(|error| AppError::io("write Worker scaffold", &error))?;
    }
    Ok(json!({"path":path,"worker_name":"afmail-inbound","bucket":"afmail-spool"}))
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    /// Every file of the template project except what its own `.gitignore`
    /// excludes (dependency installs, build outputs, local secrets).
    fn project_files(dir: &Path, root: &Path, out: &mut Vec<String>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            let name = path.file_name().unwrap().to_str().unwrap();
            if path.is_dir() {
                if !matches!(name, "node_modules" | ".wrangler" | ".cloudflare" | "dist") {
                    project_files(&path, root, out);
                }
            } else if !name.starts_with(".env") && !name.starts_with(".dev.vars") {
                let relative = path.strip_prefix(root).unwrap();
                out.push(relative.to_str().unwrap().replace('\\', "/"));
            }
        }
    }

    #[test]
    fn embedded_project_lists_exactly_the_template_files() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("templates/cloudflare-worker");
        let mut on_disk = Vec::new();
        project_files(&root, &root, &mut on_disk);
        on_disk.sort();
        let mut embedded: Vec<_> = super::PROJECT
            .iter()
            .map(|(name, _)| name.to_string())
            .collect();
        embedded.sort();
        assert_eq!(embedded, on_disk);
    }
}
