import { useCallback, useEffect, useRef, useState } from "react";

import { getJsonSetting, setSetting } from "../ipc";

/**
 * Every view preference Lore stores, in one table that Settings, the palette and the
 * More menu all read: its stored key, its title, its default, and its choices in menu
 * order.
 */
export const PREFERENCES = {
  theme: {
    // The key earlier versions stored the theme under, so a saved choice survives.
    key: "theme",
    title: "Appearance",
    fallback: "system",
    choices: [
      { value: "system", label: "System" },
      { value: "light", label: "Light" },
      { value: "dark", label: "Dark" },
    ],
  },
  density: {
    key: "ui.density",
    title: "Density",
    // Comfortable is the spec's 28px sidebar row and 52px list row.
    fallback: "comfortable",
    choices: [
      { value: "compact", label: "Compact" },
      { value: "comfortable", label: "Comfortable" },
      { value: "spacious", label: "Spacious" },
    ],
  },
  toolCalls: {
    key: "ui.toolCalls",
    title: "Show Tool Calls",
    fallback: "collapsed",
    choices: [
      { value: "collapsed", label: "Collapsed" },
      { value: "expanded", label: "Expanded" },
    ],
  },
} as const;

export type PreferenceName = keyof typeof PREFERENCES;
export type Preferences = { [K in PreferenceName]: (typeof PREFERENCES)[K]["choices"][number]["value"] };
/** Choose a value for a preference. A value that is not one of its choices is ignored. */
export type SetPreference = (name: PreferenceName, value: string) => void;

/** "Show tool calls" (spec 9.4): one line per call, or every call opened to its input and output. */
export type ToolCallMode = Preferences["toolCalls"];

const NAMES = Object.keys(PREFERENCES) as PreferenceName[];

const DEFAULTS = Object.fromEntries(NAMES.map((name) => [name, PREFERENCES[name].fallback])) as Preferences;

function isChoice<K extends PreferenceName>(name: K, value: unknown): value is Preferences[K] {
  return PREFERENCES[name].choices.some((choice) => choice.value === value);
}

/**
 * The stored preferences, read once at launch. A choice made before its read returns
 * wins. Theme and density apply to the root element (`data-theme`, `data-density`),
 * which tokens.css reads; the default leaves the attribute off. A failure to read or
 * write only costs the preference, so errors are dropped.
 */
export function usePreferences(): [Preferences, SetPreference] {
  const [prefs, setPrefs] = useState<Preferences>(DEFAULTS);
  const chosen = useRef(new Set<PreferenceName>());

  useEffect(() => {
    let live = true;
    for (const name of NAMES) {
      getJsonSetting<string>(PREFERENCES[name].key).then(
        (stored) => {
          if (live && !chosen.current.has(name) && isChoice(name, stored)) setPrefs((prev) => ({ ...prev, [name]: stored }));
        },
        () => {},
      );
    }
    return () => {
      live = false;
    };
  }, []);

  const { theme, density } = prefs;
  useEffect(() => {
    const root = document.documentElement;
    for (const [name, value] of [["theme", theme], ["density", density]] as const) {
      if (value === DEFAULTS[name]) delete root.dataset[name];
      else root.dataset[name] = value;
    }
  }, [theme, density]);

  const set = useCallback<SetPreference>((name, value) => {
    if (!isChoice(name, value)) return;
    chosen.current.add(name);
    setPrefs((prev) => ({ ...prev, [name]: value }));
    setSetting(PREFERENCES[name].key, JSON.stringify(value)).catch(() => {});
  }, []);

  return [prefs, set];
}
