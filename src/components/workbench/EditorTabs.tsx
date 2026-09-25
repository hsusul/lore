import { useEffect, useRef, type KeyboardEvent, type ReactNode } from "react";

import { CloseIcon, FileIcon } from "./icons";
import { baseName, domId, shortcut, type Tab } from "./state";

type Props = {
  tabs: Tab[];
  activeKey: string | null;
  taskTitle: (taskId: string) => string;
  onActivate: (key: string) => void;
  onClose: (key: string) => void;
  renderTab: (tab: Tab, active: boolean) => ReactNode;
  /** Shown when no file or diff is active (welcome or the selected agent). */
  stage: ReactNode;
};

export function tabLabel(tab: Tab, taskTitle: (taskId: string) => string): string {
  switch (tab.kind) {
    case "file":
      return baseName(tab.relPath);
    case "agent":
      return `Agent: ${taskTitle(tab.taskId)}`;
  }
}

/** Tabs for open files. The agent stage is not a tab. */
export default function EditorTabs({ tabs, activeKey, taskTitle, onActivate, onClose, renderTab, stage }: Props) {
  const tabRefs = useRef(new Map<string, HTMLDivElement>());
  const fileTabs = tabs.filter((t) => t.kind !== "agent");

  useEffect(() => {
    if (activeKey) tabRefs.current.get(activeKey)?.scrollIntoView?.({ block: "nearest", inline: "nearest" });
  }, [activeKey]);

  if (fileTabs.length === 0) {
    return <div className="editor">{stage}</div>;
  }

  function onKeyDown(event: KeyboardEvent<HTMLDivElement>) {
    const index = fileTabs.findIndex((t) => t.key === activeKey);
    let next = -1;
    if (event.key === "ArrowRight") next = (index + 1) % fileTabs.length;
    else if (event.key === "ArrowLeft") next = (index - 1 + fileTabs.length) % fileTabs.length;
    else if (event.key === "Home") next = 0;
    else if (event.key === "End") next = fileTabs.length - 1;
    else return;
    event.preventDefault();
    const key = fileTabs[next].key;
    onActivate(key);
    tabRefs.current.get(key)?.focus();
  }

  return (
    <div className="editor">
      <div className="tabs" role="tablist" aria-label="Open editors" onKeyDown={onKeyDown}>
        {fileTabs.map((tab) => {
          const active = tab.key === activeKey;
          const label = tabLabel(tab, taskTitle);
          const title = tab.kind === "file" ? `${tab.root}/${tab.relPath}` : label;
          return (
            <div
              key={tab.key}
              ref={(el) => {
                if (el) tabRefs.current.set(tab.key, el);
                else tabRefs.current.delete(tab.key);
              }}
              id={domId("tab", tab.key)}
              role="tab"
              aria-selected={active}
              aria-controls={domId("tabpanel", tab.key)}
              tabIndex={active ? 0 : -1}
              title={title}
              className={`tab${active ? " tab--active" : ""}`}
              onClick={() => onActivate(tab.key)}
              onAuxClick={(e) => {
                if (e.button === 1) {
                  e.preventDefault();
                  onClose(tab.key);
                }
              }}
              onMouseDown={(e) => {
                if (e.button === 1) e.preventDefault();
              }}
            >
              <span className={`tab__icon tab__icon--${tab.kind}`}>
                <FileIcon />
              </span>
              <span className="tab__label">{label}</span>
              {tab.kind === "file" && tab.scopeLabel && <span className="tab__scope">{tab.scopeLabel}</span>}
              <button
                type="button"
                className="tab__close"
                tabIndex={-1}
                aria-label={`Close ${label}`}
                title={`Close (${shortcut("W")})`}
                onClick={(e) => {
                  e.stopPropagation();
                  onClose(tab.key);
                }}
              >
                <CloseIcon size={14} />
              </button>
            </div>
          );
        })}
      </div>
      {fileTabs.map((tab) => (
        <div
          key={tab.key}
          id={domId("tabpanel", tab.key)}
          role="tabpanel"
          aria-labelledby={domId("tab", tab.key)}
          className="editor__panel"
          hidden={tab.key !== activeKey}
        >
          {renderTab(tab, tab.key === activeKey)}
        </div>
      ))}
      {activeKey === null && <div className="editor__stage">{stage}</div>}
    </div>
  );
}
