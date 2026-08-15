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
});
