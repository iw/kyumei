//! Embeds the repository's `knowledge/` directory so the binary carries the
//! exact knowledge it was built and tested with. A runtime directory can still
//! be loaded explicitly with `KnowledgeBase::load_dir`.

use std::{
    env, fs,
    path::{Path, PathBuf},
};

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("manifest dir"));
    let root = manifest_dir
        .join("../../knowledge")
        .canonicalize()
        .expect("knowledge directory at the repository root");
    println!("cargo:rerun-if-changed={}", root.display());

    let mut files = Vec::new();
    collect(&root, &root, &mut files);
    files.sort();

    let mut generated =
        String::from("/// Embedded knowledge files: (path relative to `knowledge/`, contents).\n");
    generated.push_str("pub static FILES: &[(&str, &str)] = &[\n");
    for (relative, absolute) in &files {
        println!("cargo:rerun-if-changed={}", absolute.display());
        generated.push_str(&format!(
            "    ({relative:?}, include_str!({:?})),\n",
            absolute.display().to_string()
        ));
    }
    generated.push_str("];\n");

    let out = PathBuf::from(env::var("OUT_DIR").expect("out dir")).join("embedded.rs");
    fs::write(out, generated).expect("write embedded.rs");
}

fn collect(root: &Path, dir: &Path, files: &mut Vec<(String, PathBuf)>) {
    println!("cargo:rerun-if-changed={}", dir.display());
    let mut entries: Vec<_> = fs::read_dir(dir)
        .expect("read knowledge dir")
        .map(|e| e.expect("dir entry").path())
        .collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            collect(root, &path, files);
        } else if path.extension().is_some_and(|ext| ext == "toml") {
            let relative = path
                .strip_prefix(root)
                .expect("under root")
                .to_string_lossy()
                .replace('\\', "/");
            files.push((relative, path));
        }
    }
}
