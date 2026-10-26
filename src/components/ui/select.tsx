import * as Primitive from "@radix-ui/react-select";
import { clsx } from "clsx";
import { Check, ChevronsUpDown } from "lucide-react";
import { forwardRef, type ComponentPropsWithoutRef, type ElementRef } from "react";

import { ICON } from "./button";
import { MENU_INDICATOR, MENU_ITEM, MENU_SURFACE } from "./menu-styles";

/**
 * A macOS pop-up button (spec 9.7): the current value plus a double chevron. The menu
 * opens over the selected item, as AppKit's pop-ups do.
 */
export const Select = Primitive.Root;
export const SelectValue = Primitive.Value;

export const SelectTrigger = forwardRef<
  ElementRef<typeof Primitive.Trigger>,
  ComponentPropsWithoutRef<typeof Primitive.Trigger>
>(function SelectTrigger({ className, children, ...props }, ref) {
  return (
    <Primitive.Trigger
      ref={ref}
      className={clsx(
        "inline-flex h-control flex-none items-center gap-1 rounded-control bg-bg-button pl-2 pr-1 text-control text-label shadow-button outline-none border-contrast border-separator focus-visible:shadow-focus",
        className,
      )}
      {...props}
    >
      {children}
      <Primitive.Icon asChild>
        <ChevronsUpDown className={clsx(ICON.small, "text-label-2")} aria-hidden="true" />
      </Primitive.Icon>
    </Primitive.Trigger>
  );
});

export const SelectContent = forwardRef<
  ElementRef<typeof Primitive.Content>,
  ComponentPropsWithoutRef<typeof Primitive.Content>
>(function SelectContent({ className, children, ...props }, ref) {
  return (
    <Primitive.Portal>
      <Primitive.Content ref={ref} className={clsx(MENU_SURFACE, className)} {...props}>
        <Primitive.Viewport>{children}</Primitive.Viewport>
      </Primitive.Content>
    </Primitive.Portal>
  );
});

export const SelectItem = forwardRef<
  ElementRef<typeof Primitive.Item>,
  ComponentPropsWithoutRef<typeof Primitive.Item>
>(function SelectItem({ className, children, ...props }, ref) {
  return (
    <Primitive.Item ref={ref} className={clsx(MENU_ITEM, className)} {...props}>
      <span className={MENU_INDICATOR}>
        <Primitive.ItemIndicator>
          <Check className={ICON.small} aria-hidden="true" />
        </Primitive.ItemIndicator>
      </span>
      <Primitive.ItemText>{children}</Primitive.ItemText>
    </Primitive.Item>
  );
});
