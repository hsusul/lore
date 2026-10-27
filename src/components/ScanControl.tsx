import { clsx } from "clsx";
import { Loader, RefreshCw, TriangleAlert } from "lucide-react";
import { useEffect, useState } from "react";

import { formatAgo } from "../format";
import { scanFraction, type ScanState } from "../hooks/workspace";
import { Button, ICON } from "./ui/button";
import { Popover, PopoverClose, PopoverContent, PopoverTrigger } from "./ui/popover";
import { Tooltip } from "./ui/tooltip";

/** A scan shows an indeterminate spinner this long before switching to its progress ring (spec 9.9). */
const INDETERMINATE_MS = 3_000;

function scanSummary(scan: ScanState): string | null {
  if (scan.phase !== "done") return null;
  const p = scan.progress;
  return `${p.ingested} new or changed, ${p.skipped} unchanged${p.failed > 0 ? `, ${p.failed} unreadable` : ""}`;
}

/** Scanning words for the tooltip, the toolbar subtitle and assistive tech. */
function scanProgressText(scan: ScanState): string | null {
  if (scan.phase !== "running") return null;
  const fraction = scanFraction(scan.progress);
  return fraction ? `Scanning ${fraction.done} of ${fraction.total}` : "Looking for sessions…";
}

function ProgressRing({ percent }: { percent: number }) {
  return (
    <svg viewBox="0 0 16 16" className={ICON.body} aria-hidden="true">
      <circle cx="8" cy="8" r="6.5" fill="none" strokeWidth="2" className="stroke-fill" />
      <circle
        cx="8"
        cy="8"
        r="6.5"
        fill="none"
        strokeWidth="2"
        strokeLinecap="round"
        pathLength={100}
        strokeDasharray={`${percent} 100`}
        transform="rotate(-90 8 8)"
        className="stroke-label-2"
      />
    </svg>
  );
}

/**
 * Rescan and the scan state, which replace the status bar (spec 9.9, 18.8). The icon turns
 * into a spinner, then a progress ring; a failure adds a warning that explains itself.
 * The button never changes size, and a polite live region announces each phase. The
 * warning sits before Rescan, so nothing after it moves when it appears (spec 17.23).
 */
export default function ScanControl({
  scan,
  lastScanAt,
  onRescan,
  refreshError,
  onRefresh,
}: {
  scan: ScanState;
  lastScanAt: number | null;
  onRescan: () => void;
  /** Why the last reload of the archive after a change failed, or null. */
  refreshError: string | null;
  onRefresh: () => void;
}) {
  const running = scan.phase === "running";
  const [determinate, setDeterminate] = useState(false);
  useEffect(() => {
    if (!running) return;
    const timer = setTimeout(() => setDeterminate(true), INDETERMINATE_MS);
    return () => {
      clearTimeout(timer);
      setDeterminate(false);
    };
  }, [running]);

  const fraction = running ? scanFraction(scan.progress) : null;
  const percent = fraction ? Math.round((fraction.done / fraction.total) * 100) : 0;
  const progress = scanProgressText(scan);
  const summary = scanSummary(scan);
  // A failed scan outranks a failed reload: rescanning reloads the archive too.
  const warning =
    scan.phase === "failed"
      ? { label: "Scan failed", tooltip: "The last scan failed", title: "The scan did not finish.", error: scan.error, onRetry: onRescan }
      : refreshError !== null
        ? { label: "Archive not updated", tooltip: "The archive could not be reloaded", title: "Lore could not reload the archive.", error: refreshError, onRetry: onRefresh }
        : null;
  const idleLabel =[lastScanAt ? `Rescan (⌘R), last scanned ${formatAgo(lastScanAt)}` : "Rescan (⌘R)", summary].filter(Boolean).join(". ");

  return (
    <>
      {warning && (
        <Popover>
          <Tooltip label={warning.tooltip}>
            <PopoverTrigger asChild>
              <Button variant="toolbar" aria-label={warning.label}>
                <TriangleAlert className={clsx(ICON.toolbar, "text-orange-icon")} aria-hidden="true" />
              </Button>
            </PopoverTrigger>
          </Tooltip>
          <PopoverContent align="end" aria-label={warning.label}>
            <p className="text-headline">{warning.title}</p>
            <p className="mt-1 text-callout text-label-2">{warning.error}</p>
            <p className="mt-1 text-callout text-label-2">Your agent logs were not changed.</p>
            <div className="mt-3 flex justify-end">
              <PopoverClose asChild>
                <Button onClick={warning.onRetry}>Try Again</Button>
              </PopoverClose>
            </div>
          </PopoverContent>
        </Popover>
      )}
      <Tooltip label={progress ?? idleLabel}>
        <Button
          variant="toolbar"
          aria-label={running ? "Scanning" : "Rescan"}
          aria-keyshortcuts="Meta+R"
          aria-disabled={running || undefined}
          onClick={() => {
            if (!running) onRescan();
          }}
        >
          {!running ? (
            <RefreshCw className={ICON.toolbar} aria-hidden="true" />
          ) : determinate && fraction ? (
            <ProgressRing percent={percent} />
          ) : (
            // A running scan is busy, not disabled: the spinner keeps the active icon color.
            <Loader className={clsx(ICON.body, "animate-spin text-label-2")} aria-hidden="true" />
          )}
          {running && (
            <span
              role="progressbar"
              aria-label="Archive scan"
              aria-valuemin={0}
              aria-valuemax={100}
              aria-valuenow={fraction ? percent : undefined}
              aria-valuetext={fraction ? `${fraction.done} of ${fraction.total} sessions` : "Looking for sessions"}
              className="sr-only"
            />
          )}
        </Button>
      </Tooltip>
      <span role="status" className="sr-only">
        {running ? "Scanning" : summary ? `Scan finished: ${summary}` : ""}
      </span>
      <span role="alert" className="sr-only">
        {warning ? `${warning.label}: ${warning.error}` : ""}
      </span>
    </>
  );
}
