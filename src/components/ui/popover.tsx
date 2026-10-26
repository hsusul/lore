import * as Primitive from "@radix-ui/react-popover";
import { clsx } from "clsx";
import { forwardRef, type ComponentPropsWithoutRef, type ElementRef } from "react";

import { FLOAT_OFFSET } from "./metrics";

/** Popovers (spec 9.12): 10px radius, 12px padding, the menu elevation; their text stays selectable. */
export const Popover = Primitive.Root;
export const PopoverTrigger = Primitive.Trigger;
export const PopoverAnchor = Primitive.Anchor;
export const PopoverClose = Primitive.Close;

export const PopoverContent = forwardRef<
  ElementRef<typeof Primitive.Content>,
  ComponentPropsWithoutRef<typeof Primitive.Content>
>(function PopoverContent({ className, sideOffset = FLOAT_OFFSET, ...props }, ref) {
  return (
    <Primitive.Portal>
      <Primitive.Content
        ref={ref}
        sideOffset={sideOffset}
        className={clsx(
          "z-50 w-popover rounded-group bg-bg-popover p-3 text-body text-label shadow-menu outline-none select-text data-open:animate-popover-in data-closed:animate-popover-out",
          className,
        )}
        {...props}
      />
    </Primitive.Portal>
  );
});
