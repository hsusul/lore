import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

const getBackupSchedule = vi.fn();
const setBackupSchedule = vi.fn();
const backupNow = vi.fn();

vi.mock("../ipc", () => ({
  getBackupSchedule: () => getBackupSchedule(),
  setBackupSchedule: (interval: string, keep: number) => setBackupSchedule(interval, keep),
  backupNow: () => backupNow(),
}));

import BackupSettings from "./BackupSettings";

describe("BackupSettings", () => {
  beforeEach(() => {
    getBackupSchedule.mockReset().mockResolvedValue({ interval: "weekly", keep: 3 });
    setBackupSchedule.mockReset().mockResolvedValue(undefined);
    backupNow.mockReset().mockResolvedValue(undefined);
  });

  it("loads the current schedule and reflects it in the controls", async () => {
    render(<BackupSettings />);
    const select = (await screen.findByLabelText("Automatic backups")) as HTMLSelectElement;
    expect(select.value).toBe("weekly");
    expect((screen.getByLabelText("Keep newest") as HTMLInputElement).value).toBe("3");
  });

  it("persists a changed interval", async () => {
    render(<BackupSettings />);
    const select = await screen.findByLabelText("Automatic backups");
    fireEvent.change(select, { target: { value: "daily" } });
    await waitFor(() => expect(setBackupSchedule).toHaveBeenCalledWith("daily", 3));
  });

  it("runs an on-demand backup and reports success", async () => {
    render(<BackupSettings />);
    await screen.findByLabelText("Automatic backups");
    fireEvent.click(screen.getByRole("button", { name: /back up now/i }));
    await waitFor(() => expect(backupNow).toHaveBeenCalled());
    expect(await screen.findByText(/backup created/i)).toBeTruthy();
  });

  it("disables the retention input when backups are off", async () => {
    getBackupSchedule.mockResolvedValue({ interval: "off", keep: 5 });
    render(<BackupSettings />);
    const keep = (await screen.findByLabelText("Keep newest")) as HTMLInputElement;
    expect(keep.disabled).toBe(true);
  });
});
