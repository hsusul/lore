import * as Primitive from "@radix-ui/react-tooltip";
import { clsx } from "clsx";
import { forwardRef, type ComponentPropsWithoutRef, type ElementRef, type ReactElement, type ReactNode } from "react";

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

/** A control with a tooltip. The child must forward its ref (a Button does). */
export function Tooltip({ label, children }: { label: ReactNode; children: ReactElement }) {
  return (
    <TooltipRoot>
      <TooltipTrigger asChild>{children}</TooltipTrigger>
      <TooltipContent>{label}</TooltipContent>
    </TooltipRoot>
  );
}
