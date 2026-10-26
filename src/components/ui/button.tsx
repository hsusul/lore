import { clsx } from "clsx";
import { forwardRef, type ComponentPropsWithoutRef } from "react";

/**
 * macOS buttons (spec 9.7). `push` is the regular rounded button, `default` the one
 * accent-filled button a sheet may have, `toolbar` the 28px borderless icon button.
 * A destructive action is a push button with `destructive`, which colors the label.
 */
const VARIANTS = {
  push: "h-control gap-1 rounded-control bg-bg-button px-2.5 text-control text-label shadow-button border-contrast border-separator",
  default: "h-control gap-1 rounded-control bg-accent-fill px-2.5 text-control text-on-selected shadow-button",
  toolbar: "size-toolbar-btn rounded-control text-label-2 hover:bg-fill-2 aria-disabled:text-label-3 aria-disabled:hover:bg-transparent",
} as const;

export type ButtonVariant = keyof typeof VARIANTS;

export const Button = forwardRef<
  HTMLButtonElement,
  ComponentPropsWithoutRef<"button"> & { variant?: ButtonVariant; destructive?: boolean }
>(function Button({ variant = "push", destructive = false, className, type = "button", ...props }, ref) {
  return (
    <button
      ref={ref}
      type={type}
      className={clsx(
        "inline-flex flex-none items-center justify-center whitespace-nowrap outline-none focus-visible:shadow-focus disabled:text-label-3",
        VARIANTS[variant],
        destructive && "text-red",
        className,
      )}
      {...props}
    />
  );
});

/** Lucide icon classes at the spec 12 sizes: 15 next to body text, 13 next to 11px, 17 in the toolbar. */
export const ICON = {
  body: "size-icon stroke-icon flex-none",
  small: "size-icon-sm stroke-icon-sm flex-none",
  toolbar: "size-icon-lg stroke-icon-lg flex-none",
} as const;
