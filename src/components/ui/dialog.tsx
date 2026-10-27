import * as Primitive from "@radix-ui/react-dialog";
import { clsx } from "clsx";
import { forwardRef, type ComponentPropsWithoutRef, type ElementRef } from "react";

/**
 * A macOS sheet (spec 9.10): attached under the toolbar, 12px radius, the sheet
 * elevation over a light scrim, sliding down 6px as it fades in.
 */
export const Dialog = Primitive.Root;
export const DialogTrigger = Primitive.Trigger;
export const DialogClose = Primitive.Close;

const DialogOverlay = forwardRef<
  ElementRef<typeof Primitive.Overlay>,
  ComponentPropsWithoutRef<typeof Primitive.Overlay>
>(function DialogOverlay({ className, ...props }, ref) {
  return (
    <Primitive.Overlay
      ref={ref}
      className={clsx("fixed inset-0 z-50 bg-scrim data-open:animate-popover-in data-closed:animate-popover-out", className)}
      {...props}
    />
  );
});

export const DialogContent = forwardRef<
  ElementRef<typeof Primitive.Content>,
  ComponentPropsWithoutRef<typeof Primitive.Content>
>(function DialogContent({ className, ...props }, ref) {
  return (
    <Primitive.Portal>
      <DialogOverlay />
      <Primitive.Content
        ref={ref}
        className={clsx(
          "fixed inset-x-0 top-toolbar z-50 mx-auto mt-2 rounded-sheet bg-bg-popover text-body text-label shadow-sheet outline-none data-open:animate-sheet-in data-closed:animate-sheet-out",
          className,
        )}
        {...props}
      />
    </Primitive.Portal>
  );
});

export const DialogTitle = forwardRef<
  ElementRef<typeof Primitive.Title>,
  ComponentPropsWithoutRef<typeof Primitive.Title>
>(function DialogTitle({ className, ...props }, ref) {
  return <Primitive.Title ref={ref} className={clsx("text-title3", className)} {...props} />;
});

export const DialogDescription = forwardRef<
  ElementRef<typeof Primitive.Description>,
  ComponentPropsWithoutRef<typeof Primitive.Description>
>(function DialogDescription({ className, ...props }, ref) {
  return <Primitive.Description ref={ref} className={clsx("mt-2 text-body text-label-2", className)} {...props} />;
});
