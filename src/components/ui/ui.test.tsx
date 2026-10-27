import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { createRef, useState } from "react";
import { describe, expect, it } from "vitest";

import { Button } from "./button";
import { Command, CommandDialog, CommandEmpty, CommandGroup, CommandInput, CommandItem, CommandList } from "./command";
import { ContextMenu, ContextMenuContent, ContextMenuItem, ContextMenuTrigger } from "./context-menu";
import { Dialog, DialogContent, DialogDescription, DialogTitle, DialogTrigger } from "./dialog";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuTrigger,
} from "./dropdown-menu";
import { Popover, PopoverContent, PopoverTrigger } from "./popover";
import { ScrollArea } from "./scroll-area";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "./select";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "./tabs";
import { Toggle } from "./toggle";
import { Tooltip, TooltipProvider } from "./tooltip";

describe("Dialog", () => {
  function Sheet({ contentRef }: { contentRef?: React.Ref<HTMLDivElement> }) {
    return (
      <Dialog>
        <DialogTrigger asChild>
          <Button>Forget…</Button>
        </DialogTrigger>
        <DialogContent ref={contentRef}>
          <DialogTitle>Forget this session?</DialogTitle>
          <DialogDescription>The original log stays on disk.</DialogDescription>
          <Button>Cancel</Button>
        </DialogContent>
      </Dialog>
    );
  }

  it("opens from its trigger, closes on Escape, and returns focus", async () => {
    const user = userEvent.setup();
    render(<Sheet />);
    await user.click(screen.getByRole("button", { name: "Forget…" }));

    const dialog = screen.getByRole("dialog", { name: "Forget this session?" });
    expect(dialog.contains(document.activeElement)).toBe(true);
    expect(screen.getByText("The original log stays on disk.")).toBeTruthy();

    await user.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(document.activeElement).toBe(screen.getByRole("button", { name: "Forget…" }));
  });

  it("forwards a ref to the sheet on React 18", async () => {
    const ref = createRef<HTMLDivElement>();
    render(<Sheet contentRef={ref} />);
    await userEvent.setup().click(screen.getByRole("button", { name: "Forget…" }));
    expect(ref.current).toBe(screen.getByRole("dialog"));
  });
});

describe("DropdownMenu", () => {
  function SortMenu() {
    const [sort, setSort] = useState("newest");
    const [ran, setRan] = useState("");
    return (
      <>
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <Button>Sort</Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent>
            <DropdownMenuRadioGroup value={sort} onValueChange={setSort}>
              <DropdownMenuRadioItem value="newest">Newest</DropdownMenuRadioItem>
              <DropdownMenuRadioItem value="oldest">Oldest</DropdownMenuRadioItem>
            </DropdownMenuRadioGroup>
            <DropdownMenuItem onSelect={() => setRan("rescan")}>Rescan</DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenu>
        <output>{`${sort} ${ran}`}</output>
      </>
    );
  }

  it("picks an item by keyboard and returns focus to the trigger", async () => {
    const user = userEvent.setup();
    render(<SortMenu />);
    screen.getByRole("button", { name: "Sort" }).focus();
    await user.keyboard("{Enter}");
    expect(screen.getByRole("menuitemradio", { name: "Newest" }).getAttribute("aria-checked")).toBe("true");

    await user.keyboard("{ArrowDown}{Enter}");
    expect(screen.getByRole("status").textContent).toBe("oldest ");
    await waitFor(() => expect(screen.queryByRole("menu")).toBeNull());
    expect(document.activeElement).toBe(screen.getByRole("button", { name: "Sort" }));
  });

  it("runs an item picked with the pointer", async () => {
    const user = userEvent.setup();
    render(<SortMenu />);
    await user.click(screen.getByRole("button", { name: "Sort" }));
    await user.click(screen.getByRole("menuitem", { name: "Rescan" }));
    expect(screen.getByRole("status").textContent).toBe("newest rescan");
  });
});

describe("ContextMenu", () => {
  it("opens on a secondary click and runs an item", async () => {
    const user = userEvent.setup();
    const picked: string[] = [];
    render(
      <ContextMenu>
        <ContextMenuTrigger>Release prep</ContextMenuTrigger>
        <ContextMenuContent>
          <ContextMenuItem onSelect={() => picked.push("rename")}>Rename Folder…</ContextMenuItem>
        </ContextMenuContent>
      </ContextMenu>,
    );
    await user.pointer({ keys: "[MouseRight]", target: screen.getByText("Release prep") });
    await user.click(screen.getByRole("menuitem", { name: "Rename Folder…" }));
    expect(picked).toEqual(["rename"]);
    await waitFor(() => expect(screen.queryByRole("menu")).toBeNull());
  });
});

describe("Select", () => {
  it("shows the chosen value after picking an option", async () => {
    const user = userEvent.setup();
    render(
      <Select defaultValue="relevance">
        <SelectTrigger aria-label="Sort results">
          <SelectValue />
        </SelectTrigger>
        <SelectContent>
          <SelectItem value="relevance">Best Match</SelectItem>
          <SelectItem value="newest">Newest</SelectItem>
        </SelectContent>
      </Select>,
    );
    const trigger = screen.getByRole("combobox", { name: "Sort results" });
    expect(trigger.textContent).toBe("Best Match");
    await user.click(trigger);
    await user.click(screen.getByRole("option", { name: "Newest" }));
    expect(trigger.textContent).toBe("Newest");
    expect(document.activeElement).toBe(trigger);
  });
});

describe("Tooltip", () => {
  it("names its control when focused and hides on Escape", async () => {
    const user = userEvent.setup();
    render(
      <TooltipProvider>
        <Tooltip label="Rescan (⌘R)">
          <Button variant="toolbar" aria-label="Rescan" />
        </Tooltip>
      </TooltipProvider>,
    );
    await user.tab();
    expect(document.activeElement).toBe(screen.getByRole("button", { name: "Rescan" }));
    expect((await screen.findByRole("tooltip")).textContent).toBe("Rescan (⌘R)");
    await user.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("tooltip")).toBeNull());
  });
});

describe("Popover", () => {
  it("opens from its trigger, closes on Escape, and returns focus", async () => {
    const user = userEvent.setup();
    render(
      <Popover>
        <PopoverTrigger asChild>
          <Button>Details</Button>
        </PopoverTrigger>
        <PopoverContent aria-label="Session details">
          <p>42 messages</p>
        </PopoverContent>
      </Popover>,
    );
    await user.click(screen.getByRole("button", { name: "Details" }));
    expect(screen.getByRole("dialog", { name: "Session details" }).textContent).toBe("42 messages");
    await user.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(document.activeElement).toBe(screen.getByRole("button", { name: "Details" }));
  });
});

describe("Command", () => {
  function Palette({ onRun }: { onRun: (what: string) => void }) {
    return (
      <Command label="Command palette">
        <CommandInput placeholder="Search" />
        <CommandList>
          <CommandEmpty>No Results</CommandEmpty>
          <CommandGroup heading="Actions">
            <CommandItem onSelect={() => onRun("rescan")}>Rescan</CommandItem>
            <CommandItem onSelect={() => onRun("settings")}>Settings…</CommandItem>
          </CommandGroup>
        </CommandList>
      </Command>
    );
  }

  it("filters as you type and runs the highlighted item on Enter", async () => {
    const user = userEvent.setup();
    const ran: string[] = [];
    render(<Palette onRun={(what) => ran.push(what)} />);
    await user.type(screen.getByRole("combobox", { name: "Command palette" }), "sett");
    expect(screen.getAllByRole("option").map((o) => o.textContent)).toEqual(["Settings…"]);
    await user.keyboard("{Enter}");
    expect(ran).toEqual(["settings"]);
  });

  it("says when nothing matches", async () => {
    const user = userEvent.setup();
    render(<Palette onRun={() => {}} />);
    await user.type(screen.getByRole("combobox", { name: "Command palette" }), "zzz");
    expect(screen.getByText("No Results")).toBeTruthy();
  });

  it("opens as a sheet with the input focused and closes on Escape", async () => {
    const user = userEvent.setup();
    function Host() {
      const [open, setOpen] = useState(true);
      return (
        <CommandDialog open={open} onOpenChange={setOpen} label="Command palette">
          <Palette onRun={() => {}} />
        </CommandDialog>
      );
    }
    render(<Host />);
    expect(document.activeElement).toBe(screen.getByRole("combobox", { name: "Command palette" }));
    await user.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "Command palette" })).toBeNull());
  });
});

describe("Tabs", () => {
  it("switches panels by click and by arrow key", async () => {
    const user = userEvent.setup();
    render(
      <Tabs defaultValue="conversation">
        <TabsList aria-label="Reader">
          <TabsTrigger value="conversation">Conversation</TabsTrigger>
          <TabsTrigger value="files">Files 2</TabsTrigger>
        </TabsList>
        <TabsContent value="conversation">Turns</TabsContent>
        <TabsContent value="files">Edited 2 files</TabsContent>
      </Tabs>,
    );
    expect(screen.getByRole("tabpanel").textContent).toBe("Turns");
    await user.click(screen.getByRole("tab", { name: "Files 2" }));
    expect(screen.getByRole("tabpanel").textContent).toBe("Edited 2 files");

    await user.keyboard("{ArrowLeft}");
    expect(document.activeElement).toBe(screen.getByRole("tab", { name: "Conversation" }));
    expect(screen.getByRole("tabpanel").textContent).toBe("Turns");
  });
});

describe("Toggle", () => {
  it("toggles by click and by Space", async () => {
    const user = userEvent.setup();
    render(<Toggle aria-label="Show Sidebar" />);
    const toggle = screen.getByRole("button", { name: "Show Sidebar" });
    expect(toggle.getAttribute("aria-pressed")).toBe("false");
    await user.click(toggle);
    expect(toggle.getAttribute("aria-pressed")).toBe("true");
    await user.keyboard(" ");
    expect(toggle.getAttribute("aria-pressed")).toBe("false");
  });
});

describe("ScrollArea", () => {
  it("renders its content inside a scrolling viewport", () => {
    render(
      <ScrollArea>
        <p>A long transcript</p>
      </ScrollArea>,
    );
    const text = screen.getByText("A long transcript");
    const viewport = text.closest("[data-radix-scroll-area-viewport]") as HTMLElement;
    expect(viewport.style.overflowY).toBe("scroll");
  });
});
