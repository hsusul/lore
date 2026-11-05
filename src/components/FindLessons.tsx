import { useCallback, useEffect, useState } from "react";

import { lessonAgents, type LessonAgentDto } from "../ipc";
import { Mark } from "./Mark";

const KEY = "lore.lessons-agent";

function remembered(): string | null {
  try {
    return localStorage.getItem(KEY);
  } catch {
    return null;
  }
}

function remember(id: string): void {
  try {
    localStorage.setItem(KEY, id);
  } catch {
    // Storage can be unavailable; the choice lasts until Lore quits.
  }
}

/**
 * The person's agents that can write, and the one they chose: remembered on
 * this Mac, else the first that is installed and signed in.
 */
export function useAgent() {
  const [agents, setAgents] = useState<LessonAgentDto[] | null>(null);
  const [chosen, setChosen] = useState<string | null>(remembered);
  const [error, setError] = useState<string | null>(null);

  const check = useCallback(async () => {
    setError(null);
    try {
      setAgents(await lessonAgents());
    } catch (e) {
      setError(String(e));
    }
  }, []);

  useEffect(() => {
    void check();
  }, [check]);

  const ready = (agents ?? []).filter((a) => a.installed && a.signed_in);
  const agent = ready.find((a) => a.id === chosen) ?? ready[0] ?? null;
  const choose = (id: string) => {
    setChosen(id);
    remember(id);
  };
  return { agents, ready, agent, choose, check, error };
}

/**
 * The caught-up card: which of the person's agents writes lessons, and the
 * button that starts it. An agent must be installed and signed in; when none
 * is, the card says how to sign one in.
 */
export function FindLessons({
  finding,
  onFind,
}: {
  finding: string | null;
  onFind: (agent: LessonAgentDto) => void;
}) {
  const { agents, ready, agent, choose, check, error } = useAgent();

  if (finding) {
    return (
      <div role="status">
        <Mark className="mist-mark mist-spin mist-finding" />
        <h2 className="mist-h2">
          Finding lessons{" "}
          <span className="mist-hw">
            {finding} is reading your corrections. This takes a few minutes.
          </span>
        </h2>
      </div>
    );
  }

  if (agents && !agent) {
    return (
      <>
        <div className="mist-kind">Sign in first</div>
        <h2 className="mist-h2">
          Sign in to Claude Code or Codex{" "}
          <span className="mist-hw">
            Lore uses your own agent to write lessons.
          </span>
        </h2>
        <ul className="mist-rc">
          {agents.map((a) => (
            <li key={a.id}>
              <span className="mist-dot" />
              <div>
                <p>{a.label}</p>
                <div className="mist-mt">
                  {a.installed ? (
                    <>
                      Run <code>{a.sign_in}</code> in Terminal
                    </>
                  ) : (
                    "Not installed"
                  )}
                </div>
              </div>
            </li>
          ))}
        </ul>
        <div className="mist-acts">
          <button
            type="button"
            className="mist-btn pri"
            onClick={() => void check()}
          >
            Check again
          </button>
        </div>
      </>
    );
  }

  return (
    <>
      <div className="mist-kind">You're caught up</div>
      <h2 className="mist-h2">
        Find new lessons{" "}
        <span className="mist-hw">
          Your agent reads your past corrections and writes them here.
        </span>
      </h2>
      {error && (
        <p className="mist-notice" role="alert">
          {error}
        </p>
      )}
      {ready.length > 1 && agent && (
        <div
          className="mist-seg"
          role="radiogroup"
          aria-label="Agent that writes lessons"
        >
          {ready.map((a) => (
            <button
              key={a.id}
              type="button"
              role="radio"
              aria-checked={a.id === agent.id}
              className={a.id === agent.id ? "sel" : ""}
              onClick={() => choose(a.id)}
            >
              {a.label}
            </button>
          ))}
        </div>
      )}
      <div className="mist-acts">
        <button
          type="button"
          className="mist-btn pri"
          disabled={!agent}
          onClick={() => agent && onFind(agent)}
        >
          {agent ? `Find lessons with ${agent.label}` : "Find lessons"}
        </button>
      </div>
    </>
  );
}
