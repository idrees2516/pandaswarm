// PandaSwarm embedded engine — shared types (mirrors crates/pandaswarm-core).

export type AgentRole =
  | "master"
  | "coordinator"
  | "planner"
  | "coder"
  | "reviewer"
  | "tester"
  | "researcher"
  | "integrator"
  | "computer_user";

export type AgentStatus =
  | "pending"
  | "running"
  | "waiting_tool"
  | "waiting_sandbox"
  | "done"
  | "failed"
  | "cancelled";

export type TaskStatus =
  | "queued"
  | "planning"
  | "running"
  | "completed"
  | "failed"
  | "cancelled";

export interface AgentSpec {
  id: string;
  name: string;
  role: AgentRole;
  goal: string;
  workspace: string;
  model: string;
  tools: string[];
  dependsOn: string[];
  maxTurns: number;
}

export interface Milestone {
  id: string;
  title: string;
  acceptance: string[];
  agentIds: string[];
}

export interface Plan {
  id: string;
  objective: string;
  milestones: Milestone[];
}

export interface ExecResult {
  stdout: string;
  stderr: string;
  exitCode: number;
  timedOut: boolean;
  durationMs: number;
  backend: string;
}

export interface TestReport {
  passed: number;
  failed: number;
  errors: number;
  skipped: number;
  durationMs: number;
  backend: string;
  rawOutput: string;
  failurePatterns: string[];
}

export interface HarnessSuggestion {
  pattern: string;
  suggestion: string;
  priority: number;
}

export interface SwarmEventPayload {
  seq: number;
  kind: string;
  payload: Record<string, unknown>;
  createdAt?: string;
}

export interface ToolCall {
  id: string;
  name: string;
  arguments: string; // JSON
}

export interface LlmResponse {
  content: string;
  toolCalls: ToolCall[];
  provider: string;
  model: string;
}

export function newId(prefix: string): string {
  const hex = Array.from({ length: 16 }, () =>
    Math.floor(Math.random() * 16).toString(16)
  ).join("");
  return `${prefix}_${hex}${Date.now().toString(36).slice(-6)}`;
}

export function nowMs(): number {
  return Date.now();
}
