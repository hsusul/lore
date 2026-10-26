import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { useState } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("../../ipc", () => ({
  listProviders: vi.fn(),
  saveProvider: vi.fn(),
  deleteProvider: vi.fn(),
  createTask: vi.fn(),
  taskActivity: vi.fn(() => Promise.resolve([])),
  taskDiff: vi.fn(() => Promise.resolve({ text: "", truncated: false })),
  onTasksChanged: vi.fn(() => Promise.resolve(() => {})),
}));

import {
  createTask,
  deleteProvider,
  listProviders,
  saveProvider,
  type ContinueTaskRequest,
  type ProviderDto,
  type TaskAgent,
} from "../../ipc";
import AgentMenu from "./AgentMenu";
import AgentView from "./AgentView";
import NewAgentForm from "./NewAgentForm";
import ProvidersDialog from "./ProvidersDialog";
import { task } from "./testTask";

const PROVIDERS: ProviderDto[] = [
  {
    id: "ollama",
    name: "Ollama",
    kind: "ollama",
    base_url: null,
    models: ["gpt-oss:20b", "qwen3-coder:30b"],
    has_api_key: false,
  },
  {
    id: "openrouter",
    name: "OpenRouter",
    kind: "custom",
    base_url: "https://openrouter.ai/api/v1",
    models: ["qwen/qwen3-coder"],
    has_api_key: true,
  },
];

beforeEach(() => {
  vi.mocked(listProviders).mockReset();
  vi.mocked(listProviders).mockResolvedValue(PROVIDERS);
  vi.mocked(saveProvider).mockReset();
  vi.mocked(saveProvider).mockImplementation((request) =>
    Promise.resolve({ ...PROVIDERS[1], ...request, id: request.id ?? "new", base_url: request.base_url ?? null }),
  );
  vi.mocked(deleteProvider).mockReset();
  vi.mocked(deleteProvider).mockResolvedValue(undefined);
  vi.mocked(createTask).mockReset();
});

function Harness({ initialAgent = "claude_code" }: { initialAgent?: TaskAgent }) {
  const [agent, setAgent] = useState<TaskAgent>(initialAgent);
  const [model, setModel] = useState<string | null>(null);
  const [provider, setProvider] = useState<string | null>(null);
  return (
    <>
      <AgentMenu
        label="Agent"
        agent={agent}
        onAgentChange={setAgent}
        model={model}
        onModelChange={setModel}
        provider={provider}
        onProviderChange={setProvider}
      />
      <output aria-label="Choice">{JSON.stringify({ agent, model, provider })}</output>
    </>
  );
}

const choice = () => JSON.parse(screen.getByLabelText("Choice").textContent ?? "{}");

function openModels() {
  fireEvent.click(screen.getByLabelText("Agent"));
  fireEvent.click(screen.getByRole("menuitem", { name: /^Model\b/ }));
}

describe("model picker with saved providers", () => {
  it("lists each provider's models under Codex, and a Codex model leaves the provider", async () => {
    render(<Harness initialAgent="codex" />);
    openModels();
    const ollama = await screen.findByRole("group", { name: "Ollama" });
    expect(within(ollama).getAllByRole("radio").map((r) => r.textContent)).toEqual(["gpt-oss:20b", "qwen3-coder:30b"]);

    fireEvent.click(within(ollama).getByRole("radio", { name: "qwen3-coder:30b" }));
    expect(choice()).toEqual({ agent: "codex", model: "qwen3-coder:30b", provider: "ollama" });
    expect(screen.getByLabelText("Agent").textContent).toContain("qwen3-coder:30b");
    expect(screen.getByRole("menuitem", { name: /^Model\b/ }).textContent).toContain("qwen3-coder:30b · Ollama");

    // Picking Codex again keeps the choice; a Codex model of its own drops the provider.
    fireEvent.click(screen.getByRole("menuitem", { name: /^Agent\b/ }));
    fireEvent.click(screen.getByRole("menuitemradio", { name: "Codex" }));
    expect(choice()).toEqual({ agent: "codex", model: "qwen3-coder:30b", provider: "ollama" });
    fireEvent.click(screen.getByRole("menuitem", { name: /^Model\b/ }));
    expect(screen.getByRole("radio", { name: "qwen3-coder:30b" }).getAttribute("aria-checked")).toBe("true");
    fireEvent.click(screen.getByRole("radio", { name: "GPT-5.6-Sol" }));
    expect(choice()).toEqual({ agent: "codex", model: "gpt-5.6-sol", provider: null });
  });

  it("offers providers' models under Claude Code too, switching to Codex, and back to Claude drops them", async () => {
    render(<Harness />);
    openModels();
    const ollama = await screen.findByRole("group", { name: "Ollama, runs in Codex" });
    fireEvent.click(within(ollama).getByRole("radio", { name: "gpt-oss:20b" }));
    expect(choice()).toEqual({ agent: "codex", model: "gpt-oss:20b", provider: "ollama" });

    fireEvent.click(screen.getByRole("menuitem", { name: /^Agent\b/ }));
    fireEvent.click(screen.getByRole("menuitemradio", { name: "Claude Code" }));
    expect(choice()).toEqual({ agent: "claude_code", model: null, provider: null });
  });

  it("opens the providers dialog from the model list and returns focus to the picker", async () => {
    render(<Harness />);
    openModels();
    fireEvent.click(await screen.findByRole("menuitem", { name: "Model providers…" }));
    expect(screen.queryByRole("menu")).toBeNull();
    const dialog = screen.getByRole("dialog", { name: "Model providers" });
    expect(await within(dialog).findByText("OpenRouter")).toBeTruthy();

    fireEvent.keyDown(dialog, { key: "Escape" });
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(document.activeElement).toBe(screen.getByLabelText("Agent"));
  });
});

describe("ProvidersDialog", () => {
  async function renderDialog() {
    render(<ProvidersDialog onClose={vi.fn()} />);
    await screen.findByRole("list", { name: "Saved providers" });
    await screen.findByText("OpenRouter");
  }

  it("adds an API provider, its key going to the backend and its models deduplicated", async () => {
    await renderDialog();
    fireEvent.click(screen.getByRole("button", { name: "Add provider" }));
    const form = screen.getByRole("form", { name: "New provider" });
    fireEvent.click(within(form).getByRole("radio", { name: "Any API" }));
    // The Ollama default name gives way to one for the API.
    expect((within(form).getByLabelText("Name") as HTMLInputElement).value).toBe("");
    fireEvent.change(within(form).getByLabelText("Name"), { target: { value: " My gateway " } });
    fireEvent.change(within(form).getByLabelText("Base URL"), { target: { value: " https://llm.example.com/v1 " } });
    fireEvent.change(within(form).getByLabelText("API key"), { target: { value: " sk-test " } });
    fireEvent.change(within(form).getByLabelText("Models"), {
      target: { value: "deepseek-v3.1\nqwen/qwen3-coder, deepseek-v3.1\n" },
    });
    fireEvent.click(within(form).getByRole("button", { name: "Add provider" }));

    await waitFor(() =>
      expect(saveProvider).toHaveBeenCalledWith({
        name: "My gateway",
        kind: "custom",
        models: ["deepseek-v3.1", "qwen/qwen3-coder"],
        base_url: "https://llm.example.com/v1",
        api_key: "sk-test",
      }),
    );
    // Saved: the list reloads and the form closes.
    await waitFor(() => expect(screen.queryByRole("form")).toBeNull());
    expect(vi.mocked(listProviders).mock.calls.length).toBeGreaterThan(1);
  });

  it("checks the form before saving", async () => {
    await renderDialog();
    fireEvent.click(screen.getByRole("button", { name: "Add provider" }));
    const form = screen.getByRole("form", { name: "New provider" });
    fireEvent.change(within(form).getByLabelText("Models"), { target: { value: "gpt-oss:20b\n../etc" } });
    fireEvent.click(within(form).getByRole("button", { name: "Add provider" }));
    expect(screen.getByRole("alert").textContent).toMatch(/“\.\.\/etc” is not a model id/);

    fireEvent.click(within(form).getByRole("radio", { name: "Any API" }));
    fireEvent.change(within(form).getByLabelText("Name"), { target: { value: "Gateway" } });
    fireEvent.change(within(form).getByLabelText("Models"), { target: { value: "gpt-oss:20b" } });
    fireEvent.change(within(form).getByLabelText("Base URL"), { target: { value: "llm.example.com" } });
    fireEvent.click(within(form).getByRole("button", { name: "Add provider" }));
    expect(screen.getByRole("alert").textContent).toMatch(/base URL/);
    expect(saveProvider).not.toHaveBeenCalled();
  });

  it("keeps a saved key unless a new one is typed or it is removed", async () => {
    await renderDialog();
    fireEvent.click(screen.getByRole("button", { name: "Edit OpenRouter" }));
    let form = screen.getByRole("form", { name: "Edit OpenRouter" });
    expect(within(form).getByLabelText("API key").getAttribute("placeholder")).toMatch(/Saved/);
    fireEvent.click(within(form).getByRole("button", { name: "Save" }));
    await waitFor(() =>
      expect(saveProvider).toHaveBeenLastCalledWith({
        id: "openrouter",
        name: "OpenRouter",
        kind: "custom",
        models: ["qwen/qwen3-coder"],
        base_url: "https://openrouter.ai/api/v1",
      }),
    );

    fireEvent.click(await screen.findByRole("button", { name: "Edit OpenRouter" }));
    form = screen.getByRole("form", { name: "Edit OpenRouter" });
    fireEvent.click(within(form).getByLabelText("Remove the saved key"));
    fireEvent.click(within(form).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(saveProvider).toHaveBeenLastCalledWith(expect.objectContaining({ api_key: "" })));
  });

  it("removes a provider after a confirmation", async () => {
    await renderDialog();
    fireEvent.click(screen.getByRole("button", { name: "Remove Ollama" }));
    const confirm = screen.getByRole("group", { name: "Remove Ollama" });
    fireEvent.click(within(confirm).getByRole("button", { name: "Keep" }));
    expect(deleteProvider).not.toHaveBeenCalled();

    fireEvent.click(screen.getByRole("button", { name: "Remove Ollama" }));
    fireEvent.click(within(screen.getByRole("group", { name: "Remove Ollama" })).getByRole("button", { name: "Remove" }));
    await waitFor(() => expect(deleteProvider).toHaveBeenCalledWith("ollama"));
  });
});

describe("runs through a provider", () => {
  it("launches a new agent through the chosen provider", async () => {
    vi.mocked(createTask).mockResolvedValue(task({ agent: "codex", model: "gpt-oss:20b", provider: "ollama" }));
    render(<NewAgentForm workspace="/repo" onCreated={vi.fn()} onOpenFolder={vi.fn()} />);
    fireEvent.click(screen.getByLabelText("Agent"));
    fireEvent.click(screen.getByRole("menuitem", { name: /^Model\b/ }));
    fireEvent.click(await screen.findByRole("radio", { name: "gpt-oss:20b" }));
    fireEvent.change(screen.getByLabelText("Prompt"), { target: { value: "Fix the parser" } });
    fireEvent.click(screen.getByRole("button", { name: "Launch" }));
    await waitFor(() =>
      expect(createTask).toHaveBeenCalledWith(
        expect.objectContaining({ agent: "codex", model: "gpt-oss:20b", provider: "ollama" }),
      ),
    );
  });

  it("continues through the task's provider, or leaves it for a Codex model", async () => {
    const onContinue = vi.fn<(request: ContinueTaskRequest) => Promise<void>>(() => Promise.resolve());
    render(
      <AgentView
        taskId="t1"
        task={task({ state: "finished", agent: "codex", model: "gpt-oss:20b", provider: "ollama" })}
        onStop={vi.fn(() => Promise.resolve())}
        onDiscard={vi.fn(() => Promise.resolve())}
        onReveal={vi.fn(() => Promise.resolve())}
        onContinue={onContinue}
        onCommit={vi.fn(() => Promise.resolve())}
        onMerge={vi.fn()}
        onSelectTask={vi.fn()}
      />,
    );
    const input = screen.getByLabelText("Follow-up prompt");
    fireEvent.change(input, { target: { value: "add tests" } });
    fireEvent.keyDown(input, { key: "Enter" });
    await waitFor(() =>
      expect(onContinue).toHaveBeenCalledWith({ id: "t1", prompt: "add tests", model: "gpt-oss:20b", provider: "ollama" }),
    );

    fireEvent.click(screen.getByLabelText("Next agent"));
    fireEvent.click(screen.getByRole("menuitem", { name: /^Model\b/ }));
    fireEvent.click(await screen.findByRole("radio", { name: "GPT-5.5" }));
    fireEvent.change(input, { target: { value: "try again" } });
    fireEvent.keyDown(input, { key: "Enter" });
    await waitFor(() =>
      expect(onContinue).toHaveBeenLastCalledWith({ id: "t1", prompt: "try again", model: "gpt-5.5", provider: "" }),
    );
  });
});
