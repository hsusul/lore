// Line icons on a 16px grid (1.5px stroke, round caps), inline so there is no
// dependency. Drawn for Lore: soft corners, few strokes, ring motifs.

import type { ReactNode } from "react";

function Icon({ children, size = 16 }: { children: ReactNode; size?: number }) {
  return (
    <svg
      className="icon"
      width={size}
      height={size}
      viewBox="0 0 16 16"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.5"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
      focusable="false"
    >
      {children}
    </svg>
  );
}

/** Explorer: a folder with a small tree branch. */
export const FilesIcon = ({ size = 24 }: { size?: number }) => (
  <Icon size={size}>
    <path d="M2 5.25c0-.97.78-1.75 1.75-1.75h2.1l1.4 1.5h5c.97 0 1.75.78 1.75 1.75v4.5c0 .97-.78 1.75-1.75 1.75H3.75C2.78 13 2 12.22 2 11.25Z" />
    <path d="M6 8.75h4" />
  </Icon>
);

/** Agents: a node with an orbit, echoing the Lore mark. */
export const AgentsIcon = ({ size = 24 }: { size?: number }) => (
  <Icon size={size}>
    <circle cx="8" cy="8" r="2" />
    <path d="M13.4 6.2A5.75 5.75 0 1 0 12 12.1" />
    <circle cx="13.25" cy="9.25" r=".6" fill="currentColor" stroke="none" />
  </Icon>
);

export const RefreshIcon = () => (
  <Icon>
    <path d="M12.75 7.25A4.75 4.75 0 0 0 4.1 5" />
    <path d="M3.25 8.75A4.75 4.75 0 0 0 11.9 11" />
    <path d="M3.75 2.75V5.25h2.5M12.25 13.25v-2.5h-2.5" />
  </Icon>
);

export const CloseIcon = ({ size = 16 }: { size?: number }) => (
  <Icon size={size}>
    <path d="M4.75 4.75l6.5 6.5M11.25 4.75l-6.5 6.5" />
  </Icon>
);

export const ChevronIcon = () => (
  <Icon>
    <path d="M6.25 4.25 10 8l-3.75 3.75" />
  </Icon>
);

export const FileIcon = () => (
  <Icon>
    <path d="M4.75 2.25h4l3 3v7.25c0 .69-.56 1.25-1.25 1.25H4.75c-.69 0-1.25-.56-1.25-1.25v-9c0-.69.56-1.25 1.25-1.25Z" />
    <path d="M6 9.5h4M6 11.5h2.5" />
  </Icon>
);

export const FolderIcon = () => (
  <Icon>
    <path d="M2 5.25c0-.97.78-1.75 1.75-1.75h2.1l1.4 1.5h5c.97 0 1.75.78 1.75 1.75v4.5c0 .97-.78 1.75-1.75 1.75H3.75C2.78 13 2 12.22 2 11.25Z" />
  </Icon>
);

export const StopIcon = () => (
  <Icon>
    <rect x="4.25" y="4.25" width="7.5" height="7.5" rx="1.75" />
  </Icon>
);

/** Diff: two offset lines with plus and minus. */
export const DiffIcon = () => (
  <Icon>
    <path d="M4.5 2.5v5M2 5h5M9 11h5" />
    <path d="M11 2.75 5 13.25" />
  </Icon>
);

export const RevealIcon = () => (
  <Icon>
    <path d="M9.5 2.75h3.75V6.5M13.25 2.75 8 8" />
    <path d="M12 9.75v2.5c0 .83-.67 1.5-1.5 1.5h-6.75c-.83 0-1.5-.67-1.5-1.5V5.5c0-.83.67-1.5 1.5-1.5h2.5" />
  </Icon>
);

export const TrashIcon = () => (
  <Icon>
    <path d="M2.75 4.25h10.5M6.25 4.25v-1c0-.41.34-.75.75-.75h2c.41 0 .75.34.75.75v1" />
    <path d="M4.25 4.25l.6 8.2c.06.73.66 1.3 1.4 1.3h3.5c.74 0 1.34-.57 1.4-1.3l.6-8.2" />
  </Icon>
);

export const PlusIcon = () => (
  <Icon>
    <path d="M8 3.25v9.5M3.25 8h9.5" />
  </Icon>
);

export const BranchIcon = () => (
  <Icon size={14}>
    <circle cx="4.75" cy="3.75" r="1.5" />
    <circle cx="4.75" cy="12.25" r="1.5" />
    <circle cx="11.25" cy="5.25" r="1.5" />
    <path d="M4.75 5.25v5.5M11.25 6.75c0 2.5-2.5 3.25-6.5 4" />
  </Icon>
);

export const WarningIcon = () => (
  <Icon size={14}>
    <path d="M7.13 2.75a1 1 0 0 1 1.74 0l5.2 9.25a1 1 0 0 1-.87 1.5H2.8a1 1 0 0 1-.87-1.5z" />
    <path d="M8 6.5v2.75M8 11.25v.01" />
  </Icon>
);

export const OverlapIcon = () => (
  <Icon size={14}>
    <circle cx="6" cy="8" r="3.75" />
    <circle cx="10" cy="8" r="3.75" />
  </Icon>
);

export const MergeIcon = () => (
  <Icon size={14}>
    <circle cx="4.5" cy="3.5" r="1.5" />
    <circle cx="4.5" cy="12.5" r="1.5" />
    <circle cx="11.5" cy="9" r="1.5" />
    <path d="M4.5 5v6M4.5 5c0 3 2.5 4 5.5 4" />
  </Icon>
);

export const CommitIcon = () => (
  <Icon size={14}>
    <circle cx="8" cy="8" r="2.5" />
    <path d="M1.75 8H5.5M10.5 8h3.75" />
  </Icon>
);

export const ArrowUpIcon = () => (
  <Icon size={14}>
    <path d="M8 12.75v-9.5M4.25 7 8 3.25 11.75 7" />
  </Icon>
);

export const ArrowDownIcon = () => (
  <Icon size={14}>
    <path d="M8 3.25v9.5M4.25 9 8 12.75 11.75 9" />
  </Icon>
);

/** Merge-queue step waiting its turn: an open ring. */
export const PendingIcon = () => (
  <Icon size={14}>
    <circle cx="8" cy="8" r="5.25" strokeDasharray="2 2.4" />
  </Icon>
);

/** Merge-queue step in progress: a three-quarter arc (spun by CSS). */
export const ProgressIcon = () => (
  <Icon size={14}>
    <path d="M13.25 8A5.25 5.25 0 1 1 8 2.75" />
  </Icon>
);

export const CheckIcon = () => (
  <Icon size={14}>
    <circle cx="8" cy="8" r="5.75" />
    <path d="m5.5 8.25 1.75 1.75 3.25-3.5" />
  </Icon>
);

export const FailedIcon = () => (
  <Icon size={14}>
    <circle cx="8" cy="8" r="5.75" />
    <path d="m6 6 4 4M10 6l-4 4" />
  </Icon>
);

/** Skipped or cancelled: a ring with a bar through it. */
export const SkippedIcon = () => (
  <Icon size={14}>
    <circle cx="8" cy="8" r="5.75" />
    <path d="M5.5 8h5" />
  </Icon>
);

/** The Lore mark: a node linked to two overlapping rings. */
export function Mark() {
  return (
    <svg className="wb-titlebar__mark" viewBox="0 0 140 96" fill="none" aria-hidden="true">
      <circle cx="90" cy="48" r="30" stroke="currentColor" strokeWidth="7" strokeLinecap="round"
        strokeDasharray="176.98 11.52" transform="rotate(191 90 48)" />
      <circle cx="64" cy="48" r="20" stroke="currentColor" strokeWidth="7" strokeLinecap="round"
        strokeDasharray="117.98 7.68" transform="rotate(191 64 48)" />
      <circle cx="28" cy="48" r="7" fill="currentColor" />
    </svg>
  );
}
