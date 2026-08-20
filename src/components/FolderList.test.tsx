import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import type { FolderSummary } from "../ipc";
import FolderList from "./FolderList";

function folder(id: string, name: string, count = 0): FolderSummary {
  return { id, name, session_count: count, position: 0 };
}

function setup(overrides: Partial<Parameters<typeof FolderList>[0]> = {}) {
  const props = {
    folders: [folder("f1", "Inbox", 2)],
    selectedId: null,
    onSelect: vi.fn(),
    onCreate: vi.fn(),
    onRename: vi.fn(),
    onDelete: vi.fn(),
    onDropSession: vi.fn(),
    ...overrides,
  };
  render(<FolderList {...props} />);
  return props;
}

describe("FolderList", () => {
  it("renders folders with their thread counts", () => {
    setup();
    expect(screen.getByText("Inbox")).toBeTruthy();
    expect(screen.getByText("2")).toBeTruthy();
  });

  it("shows an empty hint when there are no folders", () => {
    setup({ folders: [] });
    expect(screen.getByText(/No folders yet/i)).toBeTruthy();
  });

  it("creates a folder on Enter and ignores blank names", () => {
    const props = setup({ folders: [] });
    fireEvent.click(screen.getByLabelText("New folder"));
    const field = screen.getByLabelText("New folder name");

    // Blank input commits nothing.
    fireEvent.keyDown(field, { key: "Enter" });
    expect(props.onCreate).not.toHaveBeenCalled();

    fireEvent.click(screen.getByLabelText("New folder"));
    const field2 = screen.getByLabelText("New folder name");
    fireEvent.change(field2, { target: { value: "  Later  " } });
    fireEvent.keyDown(field2, { key: "Enter" });
    expect(props.onCreate).toHaveBeenCalledWith("Later");
  });

  it("selects a folder on click and deletes on the ✕", () => {
    const props = setup();
    fireEvent.click(screen.getByRole("button", { name: /^Inbox/ }));
    expect(props.onSelect).toHaveBeenCalledWith("f1");

    fireEvent.click(screen.getByLabelText("Delete folder Inbox"));
    expect(props.onDelete).toHaveBeenCalledWith("f1");
  });

  it("renames a folder on F2 keyboard shortcut", () => {
    const props = setup();
    const folderBtn = screen.getByRole("button", { name: /^Inbox/ });
    fireEvent.keyDown(folderBtn, { key: "F2" });

    const editField = screen.getByLabelText("Rename folder Inbox");
    expect(editField).toBeTruthy();
    fireEvent.change(editField, { target: { value: "Archive" } });
    fireEvent.keyDown(editField, { key: "Enter" });
    expect(props.onRename).toHaveBeenCalledWith("f1", "Archive");
  });

  it("deletes a folder on Delete or Cmd+Backspace keyboard shortcut", () => {
    const props = setup();
    const folderBtn = screen.getByRole("button", { name: /^Inbox/ });
    fireEvent.keyDown(folderBtn, { key: "Delete" });
    expect(props.onDelete).toHaveBeenCalledWith("f1");

    fireEvent.keyDown(folderBtn, { key: "Backspace", metaKey: true });
    expect(props.onDelete).toHaveBeenCalledTimes(2);
  });

  it("files a dropped thread into the folder", () => {
    const props = setup();
    const row = screen.getByText("Inbox").closest("li")!;
    const data: Record<string, string> = {
      "application/x-lore-session": "sess-9",
    };
    const dataTransfer = {
      types: Object.keys(data),
      getData: (type: string) => data[type] ?? "",
      dropEffect: "",
    };
    fireEvent.drop(row, { dataTransfer });
    expect(props.onDropSession).toHaveBeenCalledWith("f1", "sess-9");
  });

  it("toggles is-dragover class on dragOver and dragLeave with valid session mime", () => {
    setup();
    const row = screen.getByText("Inbox").closest("li")!;
    const dataTransfer = {
      types: ["application/x-lore-session"],
      dropEffect: "",
    };
    fireEvent.dragOver(row, { dataTransfer });
    expect(row.classList.contains("is-dragover")).toBe(true);
    expect(dataTransfer.dropEffect).toBe("move");

    fireEvent.dragLeave(row);
    expect(row.classList.contains("is-dragover")).toBe(false);
  });

  it("cancels folder creation and renaming on Escape key", () => {
    const props = setup();
    // Cancel create
    fireEvent.click(screen.getByLabelText("New folder"));
    const createField = screen.getByLabelText("New folder name");
    fireEvent.change(createField, { target: { value: "Abandoned" } });
    fireEvent.keyDown(createField, { key: "Escape" });
    expect(props.onCreate).not.toHaveBeenCalled();
    expect(screen.queryByLabelText("New folder name")).toBeNull();

    // Cancel edit
    const folderBtn = screen.getByRole("button", { name: /^Inbox/ });
    fireEvent.keyDown(folderBtn, { key: "F2" });
    const editField = screen.getByLabelText("Rename folder Inbox");
    fireEvent.change(editField, { target: { value: "Abandoned Rename" } });
    fireEvent.keyDown(editField, { key: "Escape" });
    expect(props.onRename).not.toHaveBeenCalled();
    expect(screen.queryByLabelText("Rename folder Inbox")).toBeNull();
  });
});
