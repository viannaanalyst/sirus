//! Which skills Claude sees in a project. Its listing has a small budget (1% of the context
//! window), and past it skills arrive as a bare name, which it can't match to a task: with a large
//! catalog, skills stop being found unless the person names them. So a skill for a stack the project
//! doesn't use leaves the listing (`skillOverrides`, `user-invocable-only`): it costs nothing and
//! can still be called from the composer. The project's own word comes first: `AGENTS.md` or
//! `CLAUDE.md` naming a stack keeps its skills, and a line `Skills: a, b` keeps those by name.
use serde_json::{json, Map, Value};
use std::collections::HashSet;
use std::path::Path;

/// Stack-specific skills: a word of the skill's name, and what in a project shows it uses that.
/// A skill matching no word here is general and always stays listed.
const STACKS: &[(&[&str], &[&str])] = &[
    (
        &["capacitor", "capgo", "ionic"],
        &[
            "@capacitor/core",
            "capacitor.config.ts",
            "capacitor.config.json",
        ],
    ),
    (&["expo", "react-native"], &["expo", "react-native"]),
    (&["vue", "primevue"], &["vue", "primevue"]),
    (
        &["next-"],
        &[
            "next",
            "next.config.js",
            "next.config.ts",
            "next.config.mjs",
        ],
    ),
    (&["react-", "vercel-react"], &["react"]),
    (&["gsap"], &["gsap"]),
    (
        &["threejs", "shader-glsl", "particle-system"],
        &["three", "@react-three/fiber"],
    ),
    (
        &["tauri"],
        &["tauri.conf.json", "@tauri-apps/api", "src-tauri"],
    ),
    (&["supabase"], &["@supabase/supabase-js", "supabase"]),
    (&["n8n"], &["n8n", "n8n-workflow"]),
    (&["evolution-api"], &["evolution-api"]),
    (&["streamlit"], &["streamlit"]),
    (&["tailwind", "shadcn"], &["tailwindcss", "components.json"]),
    (
        &["playwright"],
        &["@playwright/test", "playwright", "playwright.config.ts"],
    ),
];
/// Folders read for manifests: the root and one level below, for monorepos.
const MAX_CHILDREN: usize = 64;

/// What a project shows of its stack.
#[derive(Default, Debug)]
pub struct Project {
    /// Dependency names, and the names of the files and folders at its root.
    marks: HashSet<String>,
    /// `AGENTS.md` and `CLAUDE.md`, in lower case.
    guide: String,
    /// Named in a `Skills:` line of the guide.
    named: HashSet<String>,
}

impl Project {
    pub fn read(cwd: &Path) -> Self {
        let mut project = Self::default();
        let mut folders = vec![cwd.to_path_buf()];
        if let Ok(entries) = std::fs::read_dir(cwd) {
            for entry in entries.flatten().take(MAX_CHILDREN) {
                let name = entry.file_name().to_string_lossy().into_owned();
                if entry.path().is_dir() && !name.starts_with('.') && name != "node_modules" {
                    folders.push(entry.path());
                }
                project.marks.insert(name);
            }
        }
        for folder in folders {
            if let Ok(text) = std::fs::read_to_string(folder.join("package.json")) {
                project.add_manifest(&text);
            }
            if std::fs::read_to_string(folder.join("requirements.txt"))
                .or_else(|_| std::fs::read_to_string(folder.join("pyproject.toml")))
                .is_ok_and(|text| text.contains("streamlit"))
            {
                project.marks.insert("streamlit".into());
            }
        }
        for name in ["AGENTS.md", "CLAUDE.md"] {
            if let Ok(text) = std::fs::read_to_string(cwd.join(name)) {
                project.add_guide(&text);
            }
        }
        project
    }

    fn add_manifest(&mut self, text: &str) {
        let Ok(manifest) = serde_json::from_str::<Value>(text) else {
            return;
        };
        for section in ["dependencies", "devDependencies", "peerDependencies"] {
            if let Some(names) = manifest[section].as_object() {
                self.marks.extend(names.keys().cloned());
            }
        }
    }

    fn add_guide(&mut self, text: &str) {
        for line in text.lines() {
            let bare = line.trim().trim_start_matches(['-', '*', ' ']);
            let Some(rest) = bare
                .strip_prefix("Skills:")
                .or_else(|| bare.strip_prefix("skills:"))
            else {
                self.guide.push_str(&line.to_lowercase());
                self.guide.push('\n');
                continue;
            };
            self.named.extend(
                rest.split(',')
                    .map(|name| name.trim().trim_matches('`').to_owned())
                    .filter(|name| !name.is_empty()),
            );
        }
    }

    /// Whether Claude should see this skill by itself here.
    fn keeps(&self, skill: &str) -> bool {
        if self.named.contains(skill) {
            return true;
        }
        let lower = skill.to_lowercase();
        let mut specific = false;
        for (words, marks) in STACKS {
            if !words.iter().any(|word| lower.contains(word)) {
                continue;
            }
            specific = true;
            if marks.iter().any(|mark| self.marks.contains(*mark))
                || words
                    .iter()
                    .any(|word| self.guide.contains(word.trim_end_matches('-')))
            {
                return true;
            }
        }
        !specific
    }
}

/// The `skillOverrides` of a conversation in `cwd`: skills turned off in Settings are off, and
/// those of a stack the project doesn't use can only be called by name.
pub fn overrides(
    cwd: &Path,
    skills: &[String],
    disabled: &[String],
    scoped: bool,
) -> Map<String, Value> {
    let project = scoped.then(|| Project::read(cwd));
    let mut out = Map::new();
    for skill in skills {
        if disabled.contains(skill) {
            out.insert(skill.clone(), json!("off"));
        } else if project
            .as_ref()
            .is_some_and(|project| !project.keeps(skill))
        {
            out.insert(skill.clone(), json!("user-invocable-only"));
        }
    }
    out
}

/// The names of the skills in these folders.
pub fn names_in(folders: &[&Path]) -> Vec<String> {
    let mut names: Vec<String> = folders
        .iter()
        .filter_map(|folder| std::fs::read_dir(folder).ok())
        .flat_map(|entries| entries.flatten())
        .filter(|entry| entry.path().join("SKILL.md").exists())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names.dedup();
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(package: &str, guide: &str) -> Project {
        let mut project = Project::default();
        project.add_manifest(package);
        project.add_guide(guide);
        project
    }

    #[test]
    fn a_general_skill_always_stays_and_a_stack_needs_its_project() {
        let web = project(r#"{"dependencies":{"vue":"3","gsap":"3"}}"#, "");
        assert!(web.keeps("brainstorming"));
        assert!(web.keeps("vue-best-practices"));
        assert!(web.keeps("gsap-core"));
        assert!(!web.keeps("capacitor-core:capacitor-plugins"));
        assert!(!web.keeps("n8n-code-python"));
        assert!(!web.keeps("expo-ui"));
    }

    #[test]
    fn the_project_guide_keeps_a_stack_or_a_skill_by_name() {
        let guide = project(
            "{}",
            "We ship with Supabase.\n- Skills: `expo-ui`, gsap-core\n",
        );
        assert!(guide.keeps("supabase-postgres-best-practices"));
        assert!(guide.keeps("expo-ui"));
        assert!(guide.keeps("gsap-core"));
        assert!(!guide.keeps("gsap-react"));
    }

    #[test]
    fn disabled_is_off_and_unscoped_leaves_the_rest_alone() {
        let dir = std::env::temp_dir().join(format!("skill-scope-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let skills = vec![
            "n8n-code-python".to_owned(),
            "brainstorming".to_owned(),
            "gone".to_owned(),
        ];
        let disabled = vec!["gone".to_owned()];
        let scoped = overrides(&dir, &skills, &disabled, true);
        assert_eq!(scoped["gone"], "off");
        assert_eq!(scoped["n8n-code-python"], "user-invocable-only");
        assert!(!scoped.contains_key("brainstorming"));
        let all = overrides(&dir, &skills, &disabled, false);
        assert_eq!(all.len(), 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
