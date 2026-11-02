import * as Primitive from "@radix-ui/react-toggle";
import { clsx } from "clsx";
import { forwardRef, type ComponentPropsWithoutRef, type ElementRef } from "react";

/** A borderless 28px toolbar toggle; pressed shows the `--fill` background, as AppKit's toolbar toggles do. */
export const Toggle = forwardRef<ElementRef<typeof Primitive.Root>, ComponentPropsWithoutRef<typeof Primitive.Root>>(
  function Toggle({ className, ...props }, ref) {
    return (
      <Primitive.Root
        ref={ref}
        className={clsx(
          "inline-flex size-toolbar-btn flex-none items-center justify-center rounded-control text-label-2 outline-none hover:bg-fill-2 focus-visible:shadow-focus data-on:bg-fill data-on:text-label",
          className,
        )}
        {...props}
      />
    );
  },
);
