//! The bundled skill catalog is generated from the shipped directory, so adding
//! a skill never requires a second list of names in Rust.
use std::{env, fs, path::PathBuf};
fn main() {
    let root = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("manifest directory"))
        .join("../../skills");
    println!("cargo:rerun-if-changed={}", root.display());
    let mut dirs: Vec<_> = fs::read_dir(&root)
        .expect("bundled skills directory")
        .flatten()
        .filter(|entry| entry.path().join("SKILL.md").is_file())
        .collect();
    dirs.sort_by_key(|entry| entry.file_name());
    let mut code = String::from("pub const BUNDLED_SKILLS: &[BundledSkill] = &[\n");
    for dir in dirs {
        let path = dir
            .path()
            .join("SKILL.md")
            .canonicalize()
            .expect("bundled skill path");
        code.push_str(&format!(
            "BundledSkill {{ name: {:?}, content: include_str!({:?}) }},\n",
            dir.file_name().to_string_lossy(),
            path
        ));
    }
    code.push_str("];\n");
    fs::write(
        PathBuf::from(env::var_os("OUT_DIR").expect("build directory")).join("bundled_skills.rs"),
        code,
    )
    .expect("write skill catalog");
}
