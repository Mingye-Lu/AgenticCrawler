//! Installs the bundled `acrawl-mcp` agent skill into a client's skills directory.
//!
//! The skill source of truth is `skills/acrawl-mcp/` at the repo root; its files are
//! embedded at compile time so the single binary can install them without a checkout.

use std::fs;
use std::io;
use std::path::Path;

pub(crate) const SKILL_NAME: &str = "acrawl-mcp";

/// `(path relative to the skill folder, contents)`.
const SKILL_FILES: &[(&str, &str)] = &[
    (
        "SKILL.md",
        include_str!("../../../skills/acrawl-mcp/SKILL.md"),
    ),
    (
        "references/recipes.md",
        include_str!("../../../skills/acrawl-mcp/references/recipes.md"),
    ),
    (
        "references/scripting.md",
        include_str!("../../../skills/acrawl-mcp/references/scripting.md"),
    ),
    (
        "references/setup.md",
        include_str!("../../../skills/acrawl-mcp/references/setup.md"),
    ),
    (
        "references/tools.md",
        include_str!("../../../skills/acrawl-mcp/references/tools.md"),
    ),
];

/// Write the skill to `<skills_dir>/acrawl-mcp/`, replacing any previous copy so
/// files dropped from a newer skill version don't linger. Returns the skill path.
pub(crate) fn install(skills_dir: &Path) -> io::Result<String> {
    let target = skills_dir.join(SKILL_NAME);
    if target.exists() {
        fs::remove_dir_all(&target)?;
    }
    for (relative, contents) in SKILL_FILES {
        let path = target.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&path, contents)?;
    }
    Ok(target.display().to_string())
}

/// Remove `<skills_dir>/acrawl-mcp/`. Returns whether anything was removed.
pub(crate) fn uninstall(skills_dir: &Path) -> io::Result<bool> {
    let target = skills_dir.join(SKILL_NAME);
    if !target.exists() {
        return Ok(false);
    }
    fs::remove_dir_all(&target)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("acrawl-skill-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn embedded_skill_has_frontmatter_name_matching_folder() {
        let skill_md = SKILL_FILES
            .iter()
            .find(|(path, _)| *path == "SKILL.md")
            .expect("SKILL.md embedded")
            .1
            .replace("\r\n", "\n");
        assert!(skill_md.starts_with("---\n"));
        assert!(skill_md.contains(&format!("\nname: {SKILL_NAME}\n")));
    }

    #[test]
    fn install_writes_all_files_and_replaces_stale_ones() {
        let dir = temp_dir("install");
        let stale = dir.join(SKILL_NAME).join("stale.md");
        fs::create_dir_all(stale.parent().unwrap()).unwrap();
        fs::write(&stale, "old").unwrap();

        install(&dir).unwrap();

        assert!(!stale.exists());
        for (relative, contents) in SKILL_FILES {
            let written = fs::read_to_string(dir.join(SKILL_NAME).join(relative)).unwrap();
            assert_eq!(&written, contents);
        }
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn uninstall_reports_whether_it_removed_anything() {
        let dir = temp_dir("uninstall");
        assert!(!uninstall(&dir).unwrap());
        install(&dir).unwrap();
        assert!(uninstall(&dir).unwrap());
        assert!(!dir.join(SKILL_NAME).exists());
        fs::remove_dir_all(&dir).ok();
    }
}
