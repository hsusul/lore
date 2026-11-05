/**
 * What an agent's harness writes into the person's turn, shown as the person
 * would say it: a slash command as `/name args`, a skill the harness injected
 * as one quiet line, and system reminders left out.
 */
export interface Shown {
  text: string;
  /** Not something the person wrote: drawn quieter. */
  quiet: boolean;
}

const tag = (text: string, name: string) => new RegExp(`<${name}>([\\s\\S]*?)</${name}>`).exec(text)?.[1]?.trim() ?? null;
const DROPPED = /<(system-reminder|local-command-caveat|local-command-stdout)>[\s\S]*?<\/\1>/g;
const SKILL = /^Base directory for this skill:\s*(\S+)/;

export function shownTurn(raw: string): Shown | null {
  const command = tag(raw, "command-name");
  if (command) {
    const args = tag(raw, "command-args");
    const name = command.startsWith("/") ? command : `/${command}`;
    return { text: args ? `${name} ${args}` : name, quiet: false };
  }
  const skill = SKILL.exec(raw.trimStart());
  if (skill) {
    const name = skill[1].replace(/\/+$/, "").split("/").pop();
    return { text: `Loaded the ${name} skill`, quiet: true };
  }
  const text = raw.replace(DROPPED, "").trim();
  return text ? { text, quiet: false } : null;
}
