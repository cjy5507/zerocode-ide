//! The bundled skill catalog is generated from the shipped directory, so adding
//! a skill never requires a second list of names in Rust. A skill's other
//! Markdown files travel with it as its reference files.
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
        // Reference files the skill's text points to, sitting beside it: every
        // other Markdown file at the top of the skill's directory.
        let mut references: Vec<_> = fs::read_dir(dir.path())
            .expect("bundled skill directory")
            .flatten()
            .filter(|entry| {
                let name = entry.file_name();
                name != "SKILL.md"
                    && name.to_string_lossy().ends_with(".md")
                    && entry.path().is_file()
            })
            .collect();
        references.sort_by_key(|entry| entry.file_name());
        let files: String = references
            .iter()
            .map(|entry| {
                format!(
                    "({:?}, include_str!({:?})), ",
                    entry.file_name().to_string_lossy(),
                    entry
                        .path()
                        .canonicalize()
                        .expect("bundled skill reference")
                )
            })
            .collect();
        code.push_str(&format!(
            "BundledSkill {{ name: {:?}, content: include_str!({:?}), files: &[{files}] }},\n",
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
