import type { TaskAgent, TaskEffort } from "../../ipc";

export type ModelOption = { id: string; label: string };

/** Claude `--model` aliases from `claude --help` (2.1.270). */
export const CLAUDE_MODELS: ModelOption[] = [
  { id: "opus", label: "Opus" },
  { id: "sonnet", label: "Sonnet" },
  { id: "fable", label: "Fable" },
];

/** Common Codex `--model` ids; Default leaves Codex's own config in place. */
export const CODEX_MODELS: ModelOption[] = [
  { id: "gpt-6-astra", label: "GPT-6 Astra" },
  { id: "gpt-5", label: "GPT-5" },
  { id: "o3", label: "o3" },
];

export const EFFORTS: { id: TaskEffort; label: string; hint: string }[] = [
  { id: "low", label: "Low", hint: "Faster, cheaper replies." },
  { id: "medium", label: "Medium", hint: "Balanced reasoning." },
  { id: "high", label: "High", hint: "More reasoning for harder work." },
  { id: "extra_high", label: "Extra high", hint: "Claude xhigh; Codex uses high." },
  { id: "max", label: "Max", hint: "Most reasoning Claude offers; Codex uses high." },
];

export function modelsFor(agent: TaskAgent): ModelOption[] {
  return agent === "codex" ? CODEX_MODELS : CLAUDE_MODELS;
}

export function modelLabel(agent: TaskAgent, model: string | null | undefined): string {
  if (!model) return "Default";
  return modelsFor(agent).find((item) => item.id === model)?.label ?? model;
}

export function effortLabel(effort: TaskEffort | null | undefined): string {
  if (!effort) return "Default";
  return EFFORTS.find((item) => item.id === effort)?.label ?? effort;
}

/** Trigger text: model and effort when chosen, otherwise the agent name. */
export function pickerLabel(
  agentLabel: string,
  agent: TaskAgent,
  model: string | null | undefined,
  effort: TaskEffort | null | undefined,
): string {
  const modelText = model ? modelLabel(agent, model) : null;
  const effortText = effort ? effortLabel(effort) : null;
  if (modelText && effortText) return `${modelText} · ${effortText}`;
  if (modelText) return modelText;
  if (effortText) return `${agentLabel} · ${effortText}`;
  return agentLabel;
}
