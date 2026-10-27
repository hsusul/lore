import { ChevronDown, Copy, Download, Ellipsis, FolderInput } from "lucide-react";

import type { ToolCallMode } from "../hooks/usePreferences";
import type { FolderSummary } from "../ipc";
import { Button, ICON } from "./ui/button";
import {
  ContextMenuItem,
  ContextMenuSeparator,
  ContextMenuSub,
  ContextMenuSubContent,
  ContextMenuSubTrigger,
} from "./ui/context-menu";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
  MenuTrailing,
} from "./ui/dropdown-menu";
import { Tooltip } from "./ui/tooltip";

export interface ReaderActions {
  copyMarkdown: (id: string) => void;
  saveFile: (id: string) => void;
  file: (id: string, folderId: string | null) => void;
  forget: (id: string, title: string) => void;
}

/** A confirmation or a failure a toolbar control shows in its own tooltip ("Copied", spec 9.12). */
export interface ActionNote {
  control: "copy" | "file" | "save";
  text: string;
}

/**
 * A session row's context menu (spec 9.12): the reader's actions for that row's session,
 * which need not be the open one.
 */
export function SessionMenuItems({
  session,
  folders,
  actions,
}: {
  session: { id: string; title: string };
  folders: FolderSummary[];
  actions: ReaderActions;
}) {
  return (
    <>
      <ContextMenuItem onSelect={() => actions.copyMarkdown(session.id)}>Copy as Markdown</ContextMenuItem>
      <ContextMenuItem onSelect={() => actions.saveFile(session.id)}>Save as File…</ContextMenuItem>
      <ContextMenuSub>
        <ContextMenuSubTrigger>File in Folder</ContextMenuSubTrigger>
        <ContextMenuSubContent>
          {folders.length === 0 ? (
            <ContextMenuItem disabled>No Folders</ContextMenuItem>
          ) : (
            folders.map((folder) => (
              <ContextMenuItem key={folder.id} onSelect={() => actions.file(session.id, folder.id)}>
                <span className="truncate">{folder.name}</span>
              </ContextMenuItem>
            ))
          )}
          <ContextMenuSeparator />
          <ContextMenuItem onSelect={() => actions.file(session.id, null)}>Remove from Folder</ContextMenuItem>
        </ContextMenuSubContent>
      </ContextMenuSub>
      <ContextMenuSeparator />
      <ContextMenuItem onSelect={() => actions.forget(session.id, session.title)}>Forget Session…</ContextMenuItem>
    </>
  );
}

/**
 * The reader's actions in the toolbar over it (spec 9.3): Copy as Markdown, File in
 * Folder, Save as File…, and a More menu with the tool-call display and Forget Session…
 * last. They stay in place, disabled, while no session is open.
 */
export default function ReaderToolbar({
  session,
  folders,
  actions,
  note,
  toolCalls,
  onToolCalls,
}: {
  session: { id: string; title: string } | null;
  folders: FolderSummary[];
  actions: ReaderActions;
  note: ActionNote | null;
  toolCalls: ToolCallMode;
  onToolCalls: (mode: ToolCallMode) => void;
}) {
  const off = session === null;
  const noteFor = (control: ActionNote["control"]) => (note?.control === control ? note.text : null);
  return (
    <div className="-ml-1.5 flex flex-none items-center gap-1">
      <Tooltip label="Copy as Markdown (⇧⌘C)" note={noteFor("copy")}>
        <Button
          variant="toolbar"
          aria-label="Copy as Markdown"
          aria-keyshortcuts="Shift+Meta+C"
          disabled={off}
          onClick={() => session && actions.copyMarkdown(session.id)}
        >
          <Copy className={ICON.toolbar} aria-hidden="true" />
        </Button>
      </Tooltip>
      <DropdownMenu>
        <Tooltip label="File in Folder" note={noteFor("file")}>
          <DropdownMenuTrigger asChild>
            <Button variant="toolbar-menu" aria-label="File in Folder" disabled={off}>
              <FolderInput className={ICON.toolbar} aria-hidden="true" />
              <ChevronDown className={ICON.small} aria-hidden="true" />
            </Button>
          </DropdownMenuTrigger>
        </Tooltip>
        <DropdownMenuContent>
          {folders.length === 0 ? (
            <DropdownMenuItem disabled>No Folders</DropdownMenuItem>
          ) : (
            folders.map((folder) => (
              <DropdownMenuItem key={folder.id} onSelect={() => session && actions.file(session.id, folder.id)}>
                <span className="truncate">{folder.name}</span>
              </DropdownMenuItem>
            ))
          )}
          <DropdownMenuSeparator />
          <DropdownMenuItem onSelect={() => session && actions.file(session.id, null)}>Remove from Folder</DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
      <Tooltip label="Save as File… (⌘S)" note={noteFor("save")}>
        <Button
          variant="toolbar"
          aria-label="Save as File…"
          aria-keyshortcuts="Meta+S"
          disabled={off}
          onClick={() => session && actions.saveFile(session.id)}
        >
          <Download className={ICON.toolbar} aria-hidden="true" />
        </Button>
      </Tooltip>
      <DropdownMenu>
        <Tooltip label="More">
          <DropdownMenuTrigger asChild>
            <Button variant="toolbar" aria-label="More">
              <Ellipsis className={ICON.toolbar} aria-hidden="true" />
            </Button>
          </DropdownMenuTrigger>
        </Tooltip>
        <DropdownMenuContent>
          <DropdownMenuLabel>Show Tool Calls</DropdownMenuLabel>
          <DropdownMenuRadioGroup value={toolCalls} onValueChange={(value) => onToolCalls(value as ToolCallMode)}>
            <DropdownMenuRadioItem value="collapsed">Collapsed</DropdownMenuRadioItem>
            <DropdownMenuRadioItem value="expanded">Expanded</DropdownMenuRadioItem>
          </DropdownMenuRadioGroup>
          <DropdownMenuSeparator />
          <DropdownMenuItem disabled={off} onSelect={() => session && actions.forget(session.id, session.title)}>
            Forget Session…
            <MenuTrailing>⌘⌫</MenuTrailing>
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
      <span className="sr-only" aria-live="polite">
        {note?.text ?? ""}
      </span>
    </div>
  );
}
