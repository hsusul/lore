import * as Primitive from "@radix-ui/react-scroll-area";
import { clsx } from "clsx";
import { forwardRef, type ComponentPropsWithoutRef, type ElementRef } from "react";

/** An overlay scroll area with a thin capsule thumb, like macOS overlay scrollers. */
export const ScrollArea = forwardRef<
  ElementRef<typeof Primitive.Root>,
  ComponentPropsWithoutRef<typeof Primitive.Root>
>(function ScrollArea({ className, children, ...props }, ref) {
  return (
    <Primitive.Root ref={ref} className={clsx("relative overflow-hidden", className)} {...props}>
      <Primitive.Viewport className="size-full rounded-inherit">{children}</Primitive.Viewport>
      <ScrollBar orientation="vertical" />
      <ScrollBar orientation="horizontal" />
      <Primitive.Corner />
    </Primitive.Root>
  );
});

const ScrollBar = forwardRef<
  ElementRef<typeof Primitive.Scrollbar>,
  ComponentPropsWithoutRef<typeof Primitive.Scrollbar>
>(function ScrollBar({ className, orientation = "vertical", ...props }, ref) {
  return (
    <Primitive.Scrollbar
      ref={ref}
      orientation={orientation}
      className={clsx(
        "flex touch-none select-none p-0.5 data-vertical:h-full data-vertical:w-2.5 data-horizontal:h-2.5 data-horizontal:flex-col",
        className,
      )}
      {...props}
    >
      <Primitive.Thumb className="relative flex-1 rounded-capsule bg-label-3" />
    </Primitive.Scrollbar>
  );
});
