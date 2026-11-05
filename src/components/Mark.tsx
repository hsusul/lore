/** The rings of the logo (assets/brand/lore-mark.svg), drawn in the current color. */
export function Mark({ className = "mist-mark" }: { className?: string }) {
  return (
    <svg viewBox="18 14 108 68" className={className} aria-hidden="true">
      <g className="r2">
        <circle
          cx="90"
          cy="48"
          r="30"
          fill="none"
          stroke="currentColor"
          strokeWidth="7"
          strokeLinecap="round"
          strokeDasharray="176.98 11.52"
          transform="rotate(191 90 48)"
        />
      </g>
      <g className="r1">
        <circle
          cx="64"
          cy="48"
          r="20"
          fill="none"
          stroke="currentColor"
          strokeWidth="7"
          strokeLinecap="round"
          strokeDasharray="117.98 7.68"
          transform="rotate(191 64 48)"
        />
      </g>
      <circle cx="28" cy="48" r="7" fill="currentColor" />
    </svg>
  );
}
