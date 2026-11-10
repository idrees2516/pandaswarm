//! Swarm planning: task → milestones → agent specs.
//!
//! The LLM path asks the model for strict JSON; any failure (no key, bad
//! JSON, provider down) falls back to a deterministic heuristic that still
//! produces a sensible swarm — the platform never blocks on the planner.

use anyhow::Result;
use pandaswarm_core::{
    new_id, prefix, AgentRole, AgentSpec, Milestone, Plan, TaskRecord, TaskStatus,
};
use pandaswarm_llm::{extract_json, ChatMessage, ChatRequest, LlmProvider};
use serde_json::json;
use std::sync::Arc;

const MAX_CODERS: usize = 4;

/// Ask the LLM for a plan + swarm; fall back to the heuristic on any error.
pub async fn plan_swarm(
    task: &TaskRecord,
    provider: Arc<dyn LlmProvider>,
    language: &str,
) -> Result<(Plan, Vec<AgentSpec>)> {
    let req = ChatRequest::new(
        "",
        vec![
            ChatMessage::system(
                "You are PANDASWARM_PLANNER. Decompose the task into 2-5 milestones and a \
                 swarm of agents. Reply with ONLY a JSON object: \
                 {\"objective\": string, \"milestones\": [{\"title\": string, \"acceptance\": [string]}]}",
            ),
            ChatMessage::user(json!({"task": task.prompt}).to_string()),
        ],
    );
    match provider.chat(&req).await {
        Ok(resp) => {
            if let Some(v) = extract_json(&resp.content) {
                if let Ok(plan) = json_to_plan(task, &v, language) {
                    return Ok(plan);
                }
            }
            let _ = provider; // fall through to heuristic
        }
        Err(_) => {}
    }
    Ok(heuristic_plan(task, language))
}

fn json_to_plan(task: &TaskRecord, v: &serde_json::Value, language: &str) -> Result<(Plan, Vec<AgentSpec>)> {
    let mut milestones = Vec::new();
    let arr = v["milestones"].as_array().cloned().unwrap_or_default();
    if arr.is_empty() {
        return Err(anyhow::anyhow!("no milestones in LLM plan"));
    }
    let mut agent_specs: Vec<AgentSpec> = Vec::new();
    for m in arr.iter().take(5) {
        let title = m["title"].as_str().unwrap_or("milestone").to_string();
        let acceptance = m["acceptance"]
            .as_array()
            .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
            .unwrap_or_else(|| vec!["criteria met".into()]);
        let ms_id = new_id(prefix::MILESTONE);
        // one coder per milestone
        let agent_id = new_id(prefix::AGENT);
        let idx = agent_specs.len() + 1;
        agent_specs.push(AgentSpec {
            id: agent_id.clone(),
            name: format!("coder-{idx}"),
            role: AgentRole::Coder,
            goal: title.clone(),
            workspace: format!("swarm/coder-{idx}"),
            model: String::new(),
            tools: vec![
                "write_file".into(),
                "read_file".into(),
                "sandbox_run".into(),
                "rag_search".into(),
                "web_search".into(),
            ],
            depends_on: vec![],
            max_turns: 6,
        });
        milestones.push(Milestone {
            id: ms_id,
            title,
            acceptance,
            agent_ids: vec![agent_id],
        });
    }
    // tester after coders
    let tester_id = new_id(prefix::AGENT);
    let coder_ids: Vec<String> = agent_specs.iter().map(|a| a.id.clone()).collect();
    agent_specs.push(AgentSpec {
        id: tester_id.clone(),
        name: "tester".into(),
        role: AgentRole::Tester,
        goal: format!("write and run the test suite for: {}", v["objective"].as_str().unwrap_or(&task.prompt)),
        workspace: "swarm/tester".into(),
        model: String::new(),
        tools: vec!["write_file".into(), "sandbox_run".into(), "read_file".into()],
        depends_on: coder_ids.clone(),
        max_turns: 4,
    });
    for m in milestones.iter_mut() {
        m.agent_ids.push(tester_id.clone());
    }
    // reviewer parallel to tester
    agent_specs.push(AgentSpec {
        id: new_id(prefix::AGENT),
        name: "reviewer".into(),
        role: AgentRole::Reviewer,
        goal: "review the produced code for correctness and clarity".into(),
        workspace: "swarm/reviewer".into(),
        model: String::new(),
        tools: vec!["read_file".into(), "rag_search".into()],
        depends_on: coder_ids,
        max_turns: 2,
    });
    // integrator last
    agent_specs.push(AgentSpec {
        id: new_id(prefix::AGENT),
        name: "integrator".into(),
        role: AgentRole::Integrator,
        goal: "merge accepted work into the session workspace and record a summary".into(),
        workspace: "master".into(),
        model: String::new(),
        tools: vec!["read_file".into(), "write_file".into(), "sandbox_run".into()],
        depends_on: vec![tester_id],
        max_turns: 2,
    });

    let plan = Plan {
        id: new_id(prefix::PLAN),
        task_id: task.id.clone(),
        objective: v["objective"].as_str().unwrap_or(&task.prompt).to_string(),
        milestones,
        created_ms: pandaswarm_core::now_ms(),
    };
    Ok((plan, agent_specs))
}

/// Deterministic fallback planner — keyword-driven milestones, standard swarm.
pub fn heuristic_plan(task: &TaskRecord, language: &str) -> (Plan, Vec<AgentSpec>) {
    let _ = language;
    let lower = task.prompt.to_lowercase();
    let mut topics: Vec<String> = Vec::new();
    for (kw, topic) in [
        ("fibonacci", "fibonacci module"),
        ("palindrome", "palindrome checks"),
        ("sort", "sorting"),
        ("prime", "prime helpers"),
        ("search", "search"),
        ("api", "API client"),
        ("scraper", "scraper"),
        ("parser", "parser"),
        ("util", "utilities"),
    ] {
        if lower.contains(kw) && topics.len() < MAX_CODERS {
            topics.push(topic.to_string());
        }
    }
    if topics.is_empty() {
        // split on commas / "and" / sentences
        let rough: Vec<String> = task
            .prompt
            .split(|c: char| c == ',' || c == '.' || c == ';')
            .map(|s| s.trim())
            .filter(|s| s.len() > 3)
            .take(MAX_CODERS)
            .map(String::from)
            .collect();
        topics = if rough.is_empty() { vec![task.prompt.clone()] } else { rough };
    }

    let mut milestones = Vec::new();
    let mut agents = Vec::new();
    let mut coder_ids = Vec::new();
    for (i, topic) in topics.into_iter().enumerate() {
        let agent_id = new_id(prefix::AGENT);
        coder_ids.push(agent_id.clone());
        agents.push(AgentSpec {
            id: agent_id.clone(),
            name: format!("coder-{}", i + 1),
            role: AgentRole::Coder,
            goal: format!("implement {topic}"),
            workspace: format!("swarm/coder-{}", i + 1),
            model: String::new(),
            tools: vec![
                "write_file".into(),
                "read_file".into(),
                "sandbox_run".into(),
                "rag_search".into(),
                "web_search".into(),
            ],
            depends_on: vec![],
            max_turns: 6,
        });
        milestones.push(Milestone {
            id: new_id(prefix::MILESTONE),
            title: format!("Implement {topic}"),
            acceptance: vec![format!("{topic} works and passes its tests")],
            agent_ids: vec![agent_id],
        });
    }

    let tester_id = new_id(prefix::AGENT);
    agents.push(AgentSpec {
        id: tester_id.clone(),
        name: "tester".into(),
        role: AgentRole::Tester,
        goal: format!("write and run the test suite for: {}", task.prompt),
        workspace: "swarm/tester".into(),
        model: String::new(),
        tools: vec!["write_file".into(), "sandbox_run".into(), "read_file".into()],
        depends_on: coder_ids.clone(),
        max_turns: 4,
    });
    for m in milestones.iter_mut() {
        m.agent_ids.push(tester_id.clone());
    }
    agents.push(AgentSpec {
        id: new_id(prefix::AGENT),
        name: "reviewer".into(),
        role: AgentRole::Reviewer,
        goal: "review the produced code".into(),
        workspace: "swarm/reviewer".into(),
        model: String::new(),
        tools: vec!["read_file".into(), "rag_search".into()],
        depends_on: coder_ids,
        max_turns: 2,
    });
    agents.push(AgentSpec {
        id: new_id(prefix::AGENT),
        name: "integrator".into(),
        role: AgentRole::Integrator,
        goal: "merge accepted work and write the session summary".into(),
        workspace: "master".into(),
        model: String::new(),
        tools: vec!["read_file".into(), "write_file".into(), "sandbox_run".into()],
        depends_on: vec![tester_id],
        max_turns: 2,
    });

    (
        Plan {
            id: new_id(prefix::PLAN),
            task_id: task.id.clone(),
            objective: task.prompt.clone(),
            milestones,
            created_ms: pandaswarm_core::now_ms(),
        },
        agents,
    )
}

/// Build the initial task record for a session prompt.
pub fn new_task(session_id: &str, prompt: &str, repo: Option<&str>) -> TaskRecord {
    TaskRecord {
        id: new_id(prefix::TASK),
        session_id: session_id.into(),
        prompt: prompt.into(),
        status: TaskStatus::Queued,
        plan_id: None,
        repo: repo.map(String::from),
        created_ms: pandaswarm_core::now_ms(),
    }
}
