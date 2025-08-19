use serde::Deserialize;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Deserialize)]
pub struct WorkflowToml {
    pub name: Option<String>,
    pub description: Option<String>,
    pub steps: Vec<String>,
    #[serde(default)]
    pub step: HashMap<String, WorkflowStepToml>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WorkflowStepToml {
    pub r#type: String, // "agent" | "team"
    pub id: String,     // agent name or team name
    pub prompt: Option<String>,
    pub max_turns: Option<usize>,
}

#[derive(Debug, Clone)]
pub struct WorkflowDefinition {
    pub file: PathBuf,
    pub name: String,
    pub description: Option<String>,
    pub steps: Vec<WorkflowStep>,
}

#[derive(Debug, Clone)]
pub struct WorkflowStep {
    pub kind: StepKind,
    pub id: String,
    pub prompt: Option<String>,
    pub max_turns: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StepKind {
    Agent,
    Team,
}

pub fn discover_workflows(project_codex_dir: &Path) -> std::io::Result<Vec<String>> {
    let dir = project_codex_dir.join("workflows");
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for entry in fs::read_dir(&dir)? {
        let entry = entry?;
        let p = entry.path();
        if p.is_file() && p.extension().and_then(|s| s.to_str()) == Some("toml") {
            if let Some(stem) = p.file_stem().and_then(|s| s.to_str()) {
                out.push(stem.to_string());
            }
        }
    }
    out.sort();
    Ok(out)
}

pub fn load_workflow(project_codex_dir: &Path, name: &str) -> std::io::Result<WorkflowDefinition> {
    let file = project_codex_dir.join("workflows").join(format!("{name}.toml"));
    let raw = fs::read_to_string(&file).map_err(|e| {
        std::io::Error::new(
            e.kind(),
            format!("failed to read workflow {}: {e}", file.display()),
        )
    })?;
    let wf: WorkflowToml = toml::from_str(&raw).map_err(|e| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("failed to parse workflow {}: {e}", file.display()),
        )
    })?;

    let mut steps = Vec::new();
    for key in wf.steps.iter() {
        let st = wf.step.get(key).ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("workflow step '{key}' not defined in [step.*]"),
            )
        })?;
        let kind = match st.r#type.as_str() {
            "agent" => StepKind::Agent,
            "team" => StepKind::Team,
            other => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("unsupported step type '{other}' for step '{key}'"),
                ))
            }
        };
        steps.push(WorkflowStep {
            kind,
            id: st.id.clone(),
            prompt: st.prompt.clone(),
            max_turns: st.max_turns,
        });
    }

    Ok(WorkflowDefinition {
        file,
        name: wf.name.unwrap_or_else(|| name.to_string()),
        description: wf.description,
        steps,
    })
}

