import type { TaskAgent, TaskEffort } from "../../ipc";

export type ModelOption = {
  id: string;
  label: string;
  /** The family heading the picker files it under. */
  group?: string;
};

/**
 * Claude `--model` values. A family alias runs the newest model in that family
 * (`claude --help`, 2.1.270); a full id pins one version.
 */
export const CLAUDE_MODELS: ModelOption[] = [
  { id: "fable", label: "Fable", group: "Newest in family" },
  { id: "opus", label: "Opus", group: "Newest in family" },
  { id: "sonnet", label: "Sonnet", group: "Newest in family" },
  { id: "claude-fable-5-1", label: "Fable 5.1", group: "Fable" },
  { id: "claude-fable-5", label: "Fable 5", group: "Fable" },
  { id: "claude-opus-5-5", label: "Opus 5.5", group: "Opus" },
  { id: "claude-opus-5", label: "Opus 5", group: "Opus" },
  { id: "claude-opus-4-8", label: "Opus 4.8", group: "Opus" },
  { id: "claude-opus-4-7", label: "Opus 4.7", group: "Opus" },
  { id: "claude-opus-4-6", label: "Opus 4.6", group: "Opus" },
  { id: "claude-sonnet-5", label: "Sonnet 5", group: "Sonnet" },
  { id: "claude-sonnet-4-6", label: "Sonnet 4.6", group: "Sonnet" },
  { id: "claude-haiku-4-5", label: "Haiku 4.5", group: "Haiku" },
];

/**
 * Codex `--model` ids, in the order of Codex's own model catalog (`codex debug
 * models`, codex-cli 0.154.0). A model Codex adds later can be typed in as Other.
 */
export const CODEX_MODELS: ModelOption[] = [
  { id: "gpt-6-astra", label: "GPT-6-Astra" },
  { id: "gpt-5.6-sol", label: "GPT-5.6-Sol" },
  { id: "gpt-5.6-terra", label: "GPT-5.6-Terra" },
  { id: "gpt-5.6-luna", label: "GPT-5.6-Luna" },
  { id: "gpt-daybreak-blue-latest", label: "Daybreak Blue" },
  { id: "gpt-5.5", label: "GPT-5.5" },
];

/**
 * Whether the orchestrator will accept `raw` as a model (it checks the same in
 * `parse_model`): letters, digits, `.`, `_`, `-`, no leading dash, at most 80.
 */
export function isModelId(raw: string): boolean {
  return raw.length > 0 && raw.length <= 80 && !raw.startsWith("-") && /^[A-Za-z0-9._-]+$/.test(raw);
}

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
