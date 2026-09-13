import { useEffect, useRef, type KeyboardEvent, type ReactNode } from "react";

import { AgentsIcon, CloseIcon, DiffIcon, FileIcon } from "./icons";
import { baseName, domId, type Tab } from "./state";

type Props = {
  tabs: Tab[];
  activeKey: string | null;
  taskTitle: (taskId: string) => string;
  onActivate: (key: string) => void;
  onClose: (key: string) => void;
  renderTab: (tab: Tab) => ReactNode;
  welcome: ReactNode;
};

export function tabLabel(tab: Tab, taskTitle: (taskId: string) => string): string {
  switch (tab.kind) {
    case "file":
      return baseName(tab.relPath);
    case "diff":
      return `Diff: ${taskTitle(tab.taskId)}`;
    case "agent":
      return `Agent: ${taskTitle(tab.taskId)}`;
  }
}

/** The editor area: a VS Code style tab strip over one panel per open tab. */
export default function EditorTabs({ tabs, activeKey, taskTitle, onActivate, onClose, renderTab, welcome }: Props) {
  const tabRefs = useRef(new Map<string, HTMLDivElement>());

  useEffect(() => {
    if (activeKey) tabRefs.current.get(activeKey)?.scrollIntoView?.({ block: "nearest", inline: "nearest" });
  }, [activeKey]);

  if (tabs.length === 0) {
    return <div className="editor editor--empty">{welcome}</div>;
  }

  function onKeyDown(event: KeyboardEvent<HTMLDivElement>) {
    const index = tabs.findIndex((t) => t.key === activeKey);
    let next = -1;
    if (event.key === "ArrowRight") next = (index + 1) % tabs.length;
    else if (event.key === "ArrowLeft") next = (index - 1 + tabs.length) % tabs.length;
    else if (event.key === "Home") next = 0;
    else if (event.key === "End") next = tabs.length - 1;
    else return;
    event.preventDefault();
    const key = tabs[next].key;
    onActivate(key);
    tabRefs.current.get(key)?.focus();
  }

  return (
    <div className="editor">
      <div className="tabs" role="tablist" aria-label="Open editors" onKeyDown={onKeyDown}>
        {tabs.map((tab) => {
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
                // Stop middle-click autoscroll so aux-click can close the tab.
                if (e.button === 1) e.preventDefault();
              }}
            >
              <span className={`tab__icon tab__icon--${tab.kind}`}>
                {tab.kind === "file" ? <FileIcon /> : tab.kind === "diff" ? <DiffIcon /> : <AgentsIcon size={16} />}
              </span>
              <span className="tab__label">{label}</span>
              {tab.kind === "file" && tab.scopeLabel && <span className="tab__scope">{tab.scopeLabel}</span>}
              <button
                type="button"
                className="tab__close"
                tabIndex={-1}
                aria-label={`Close ${label}`}
                title="Close (⌘W)"
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
      {tabs.map((tab) => (
        <div
          key={tab.key}
          id={domId("tabpanel", tab.key)}
          role="tabpanel"
          aria-labelledby={domId("tab", tab.key)}
          className="editor__panel"
          hidden={tab.key !== activeKey}
        >
          {renderTab(tab)}
        </div>
      ))}
    </div>
  );
}
