import * as Primitive from "@radix-ui/react-dialog";
import { clsx } from "clsx";
import { Command as Cmdk } from "cmdk";
import { Search } from "lucide-react";
import { forwardRef, type ComponentPropsWithoutRef, type ElementRef, type ReactNode } from "react";

import { ICON } from "./button";

/**
 * A Spotlight-style palette (spec 9.8) built on cmdk: a 48px input row with no border,
 * grouped 28px rows, the active row filled with the selection color.
 */
export const Command = forwardRef<ElementRef<typeof Cmdk>, ComponentPropsWithoutRef<typeof Cmdk>>(function Command(
  { className, ...props },
  ref,
) {
  return <Cmdk ref={ref} className={clsx("flex flex-col overflow-hidden bg-bg-popover text-body text-label", className)} {...props} />;
});

export const CommandInput = forwardRef<
  ElementRef<typeof Cmdk.Input>,
  ComponentPropsWithoutRef<typeof Cmdk.Input>
>(function CommandInput({ className, ...props }, ref) {
  return (
    <div className="flex h-palette-input flex-none items-center gap-2.5 px-4 shadow-hairline-b">
      <Search className={clsx(ICON.body, "text-label-2")} aria-hidden="true" />
      <Cmdk.Input
        ref={ref}
        className={clsx("min-w-0 flex-1 bg-transparent text-title3 font-regular text-label outline-none placeholder:text-label-3", className)}
        {...props}
      />
    </div>
  );
});

export const CommandList = forwardRef<ElementRef<typeof Cmdk.List>, ComponentPropsWithoutRef<typeof Cmdk.List>>(
  function CommandList({ className, ...props }, ref) {
    return <Cmdk.List ref={ref} className={clsx("max-h-palette-list overflow-y-auto p-1.5", className)} {...props} />;
  },
);

export const CommandEmpty = forwardRef<ElementRef<typeof Cmdk.Empty>, ComponentPropsWithoutRef<typeof Cmdk.Empty>>(
  function CommandEmpty({ className, ...props }, ref) {
    return <Cmdk.Empty ref={ref} className={clsx("px-2.5 py-2 text-body text-label-2", className)} {...props} />;
  },
);

export const CommandGroup = forwardRef<ElementRef<typeof Cmdk.Group>, ComponentPropsWithoutRef<typeof Cmdk.Group>>(
  function CommandGroup({ className, ...props }, ref) {
    return (
      <Cmdk.Group
        ref={ref}
        className={clsx("cmdk-heading:px-2.5 cmdk-heading:pb-1 cmdk-heading:pt-2 cmdk-heading:text-subheadline cmdk-heading:font-semibold cmdk-heading:text-label-2", className)}
        {...props}
      />
    );
  },
);

export const CommandItem = forwardRef<ElementRef<typeof Cmdk.Item>, ComponentPropsWithoutRef<typeof Cmdk.Item>>(
  function CommandItem({ className, ...props }, ref) {
    return (
      <Cmdk.Item
        ref={ref}
        className={clsx(
          "group flex h-source-row items-center gap-2 rounded-control px-2.5 outline-none aria-selected:bg-selected aria-selected:text-on-selected aria-disabled:text-label-3",
          className,
        )}
        {...props}
      />
    );
  },
);

export const CommandSeparator = forwardRef<
  ElementRef<typeof Cmdk.Separator>,
  ComponentPropsWithoutRef<typeof Cmdk.Separator>
>(function CommandSeparator({ className, ...props }, ref) {
  return <Cmdk.Separator ref={ref} className={clsx("mx-2.5 my-1.5 h-px bg-separator", className)} {...props} />;
});

/** A shortcut in Apple glyph order, never boxed keycaps (spec 11). */
export function CommandShortcut({ children }: { children: ReactNode }) {
  return <span className="ml-auto text-subheadline tabular-nums text-label-2 group-aria-selected:text-on-selected">{children}</span>;
}

/** The palette sheet: 640px wide, its top at 18% of the window. */
export function CommandDialog({
  open,
  onOpenChange,
  label,
  children,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  label: string;
  children: ReactNode;
}) {
  return (
    <Primitive.Root open={open} onOpenChange={onOpenChange}>
      <Primitive.Portal>
        <Primitive.Overlay className="fixed inset-0 z-50 bg-scrim data-open:animate-popover-in data-closed:animate-popover-out" />
        <Primitive.Content
          aria-describedby={undefined}
          className="fixed inset-x-0 top-palette z-50 mx-auto w-palette overflow-hidden rounded-sheet shadow-sheet outline-none data-open:animate-sheet-in data-closed:animate-sheet-out"
        >
          <Primitive.Title className="sr-only">{label}</Primitive.Title>
          {children}
        </Primitive.Content>
      </Primitive.Portal>
    </Primitive.Root>
  );
}
