import * as Primitive from "@radix-ui/react-tooltip";
import { clsx } from "clsx";
import { forwardRef, useState, type ComponentPropsWithoutRef, type ElementRef, type ReactElement, type ReactNode } from "react";

import { FLOAT_OFFSET, TOOLTIP_DELAY_MS } from "./metrics";

/** System-style tooltips (spec 9.12): 11px text on the content color, 600ms delay, 8px below. */
export function TooltipProvider({ children }: { children: ReactNode }) {
  return <Primitive.Provider delayDuration={TOOLTIP_DELAY_MS}>{children}</Primitive.Provider>;
}

export const TooltipRoot = Primitive.Root;
export const TooltipTrigger = Primitive.Trigger;

export const TooltipContent = forwardRef<
  ElementRef<typeof Primitive.Content>,
  ComponentPropsWithoutRef<typeof Primitive.Content>
>(function TooltipContent({ className, side = "bottom", sideOffset = FLOAT_OFFSET, ...props }, ref) {
  return (
    <Primitive.Portal>
      <Primitive.Content
        ref={ref}
        side={side}
        sideOffset={sideOffset}
        className={clsx(
          "z-50 rounded-control bg-bg-content px-1.5 py-0.5 text-subheadline text-label shadow-button select-none animate-popover-in",
          className,
        )}
        {...props}
      />
    </Primitive.Portal>
  );
});

/**
 * A control with a tooltip. The child must forward its ref (a Button does). A `note`
 * replaces the label and holds the tooltip open, which is how a control confirms what
 * it just did ("Copied", spec 9.12) instead of a toast.
 */
export function Tooltip({ label, note = null, children }: { label: ReactNode; note?: string | null; children: ReactElement }) {
  const [hovered, setHovered] = useState(false);
  return (
    <TooltipRoot open={hovered || note !== null} onOpenChange={setHovered}>
      <TooltipTrigger asChild>{children}</TooltipTrigger>
      <TooltipContent>{note ?? label}</TooltipContent>
    </TooltipRoot>
  );
}
