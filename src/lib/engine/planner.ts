// Swarm planning: heuristic decomposition first, LLM refinement when
// available. Mirrors crates/pandaswarm-orchestrator/src/planner.rs.

import type { AgentSpec, Milestone, Plan } from "./types";
import { newId } from "./types";
import { chat } from "./llm";
import { extractJson as jsonFromText } from "./text";

const MAX_CODERS = 4;

interface PlanJson {
  objective: string;
  milestones: Array<{ title: string; acceptance?: string[] }>;
}

export async function planSwarm(prompt: string, language = "python"): Promise<{ plan: Plan; agents: AgentSpec[] }> {
  // 1. ask the LLM (any provider) for a structured plan
  let llmPlan: PlanJson | null = null;
  try {
    const resp = await chat({
      system:
        "You are PANDASWARM_PLANNER. Decompose the task into 2-5 milestones. Reply with ONLY a JSON object: " +
        '{"objective": string, "milestones": [{"title": string, "acceptance": [string]}]}',
      user: JSON.stringify({ task: prompt }),
    });
    const parsed = jsonFromText(resp.content) as PlanJson | null;
    if (parsed?.milestones?.length) llmPlan = parsed;
  } catch {
    /* heuristic below */
  }

  // 2. build milestones (LLM or heuristic)
  const topics: string[] = [];
  if (llmPlan) {
    for (const m of llmPlan.milestones.slice(0, 5)) {
      if (m.title && !/review|integrate|test/i.test(m.title)) topics.push(m.title);
    }
  }
  if (topics.length === 0) {
    const lower = prompt.toLowerCase();
    for (const [kw, topic] of [
      ["fibonacci", "fibonacci module"],
      ["palindrome", "palindrome checks"],
      ["sort", "sorting"],
      ["prime", "prime helpers"],
      ["search", "search"],
      ["api", "API client"],
      ["scraper", "scraper"],
      ["parser", "parser"],
      ["util", "utilities"],
    ] as const) {
      if (lower.includes(kw) && topics.length < MAX_CODERS) topics.push(topic);
    }
    if (topics.length === 0) {
      topics.push(
        ...prompt
          .split(/[,.;]/)
          .map((s) => s.trim())
          .filter((s) => s.length > 3)
          .slice(0, MAX_CODERS)
      );
      if (topics.length === 0) topics.push(prompt);
    }
  }

  const milestones: Milestone[] = [];
  const agents: AgentSpec[] = [];
  const coderIds: string[] = [];

  topics.slice(0, MAX_CODERS).forEach((topic, i) => {
    const agentId = newId("agent");
    coderIds.push(agentId);
    agents.push({
      id: agentId,
      name: `coder-${i + 1}`,
      role: "coder",
      goal: `implement ${topic}`,
      workspace: `swarm/coder-${i + 1}`,
      model: "",
      tools: ["write_file", "read_file", "sandbox_run", "rag_search", "web_search"],
      dependsOn: [],
      maxTurns: 6,
    });
    milestones.push({
      id: newId("mile"),
      title: `Implement ${topic}`,
      acceptance: llmPlan?.milestones.find((m) => m.title && topics.includes(m.title))?.acceptance ?? [
        `${topic} works and passes its tests`,
      ],
      agentIds: [agentId],
    });
  });

  const testerId = newId("agent");
  agents.push({
    id: testerId,
    name: "tester",
    role: "tester",
    goal: `write and run the test suite for: ${prompt}`,
    workspace: "swarm/tester",
    model: "",
    tools: ["write_file", "sandbox_run", "read_file"],
    dependsOn: coderIds,
    maxTurns: 4,
  });
  milestones.forEach((m) => m.agentIds.push(testerId));

  agents.push({
    id: newId("agent"),
    name: "reviewer",
    role: "reviewer",
    goal: "review the produced code for correctness and clarity",
    workspace: "swarm/reviewer",
    model: "",
    tools: ["read_file", "rag_search"],
    dependsOn: coderIds,
    maxTurns: 2,
  });

  agents.push({
    id: newId("agent"),
    name: "integrator",
    role: "integrator",
    goal: "merge accepted work into the session workspace and record a summary",
    workspace: "master",
    model: "",
    tools: ["read_file", "write_file", "sandbox_run"],
    dependsOn: [testerId],
    maxTurns: 2,
  });

  return {
    plan: {
      id: newId("plan"),
      objective: llmPlan?.objective ?? prompt,
      milestones,
    },
    agents,
  };
}
