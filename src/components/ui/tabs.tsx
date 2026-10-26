import * as Primitive from "@radix-ui/react-tabs";
import { clsx } from "clsx";
import { forwardRef, type ComponentPropsWithoutRef, type ElementRef } from "react";

/**
 * Tabs drawn as a macOS segmented control (spec 9.3): a 22px track in `--fill-2`; the
 * selected segment raised, ringed so it clears 3:1, and its label semibold.
 */
export const Tabs = Primitive.Root;

export const TabsList = forwardRef<ElementRef<typeof Primitive.List>, ComponentPropsWithoutRef<typeof Primitive.List>>(
  function TabsList({ className, ...props }, ref) {
    return (
      <Primitive.List
        ref={ref}
        className={clsx("inline-grid h-seg grid-flow-col auto-cols-fr gap-0.5 rounded-control bg-fill-2 p-0.5 select-none", className)}
        {...props}
      />
    );
  },
);

export const TabsTrigger = forwardRef<
  ElementRef<typeof Primitive.Trigger>,
  ComponentPropsWithoutRef<typeof Primitive.Trigger>
>(function TabsTrigger({ className, ...props }, ref) {
  return (
    <Primitive.Trigger
      ref={ref}
      className={clsx(
        "inline-flex min-w-0 items-center justify-center gap-1 whitespace-nowrap rounded-segment px-3 text-callout text-label outline-none focus-visible:shadow-focus data-active:bg-seg-on data-active:font-semibold data-active:shadow-segment",
        className,
      )}
      {...props}
    />
  );
});

export const TabsContent = forwardRef<
  ElementRef<typeof Primitive.Content>,
  ComponentPropsWithoutRef<typeof Primitive.Content>
>(function TabsContent({ className, ...props }, ref) {
  return <Primitive.Content ref={ref} className={clsx("outline-none", className)} {...props} />;
});
