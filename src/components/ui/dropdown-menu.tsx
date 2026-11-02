import * as Menu from "@radix-ui/react-dropdown-menu";
import { clsx } from "clsx";
import { Check } from "lucide-react";
import { forwardRef, type ComponentPropsWithoutRef, type ElementRef } from "react";

import { ICON } from "./button";
import { MENU_INDICATOR, MENU_ITEM, MENU_LABEL, MENU_SEPARATOR, MENU_SURFACE, MENU_TRAILING } from "./menu-styles";
import { MENU_OFFSET } from "./metrics";

export const DropdownMenu = Menu.Root;
export const DropdownMenuTrigger = Menu.Trigger;
export const DropdownMenuGroup = Menu.Group;
export const DropdownMenuRadioGroup = Menu.RadioGroup;

export const DropdownMenuContent = forwardRef<
  ElementRef<typeof Menu.Content>,
  ComponentPropsWithoutRef<typeof Menu.Content>
>(function DropdownMenuContent({ className, sideOffset = MENU_OFFSET, align = "start", ...props }, ref) {
  return (
    <Menu.Portal>
      <Menu.Content ref={ref} sideOffset={sideOffset} align={align} className={clsx(MENU_SURFACE, className)} {...props} />
    </Menu.Portal>
  );
});

export const DropdownMenuItem = forwardRef<
  ElementRef<typeof Menu.Item>,
  ComponentPropsWithoutRef<typeof Menu.Item>
>(function DropdownMenuItem({ className, ...props }, ref) {
  return <Menu.Item ref={ref} className={clsx(MENU_ITEM, className)} {...props} />;
});

export const DropdownMenuRadioItem = forwardRef<
  ElementRef<typeof Menu.RadioItem>,
  ComponentPropsWithoutRef<typeof Menu.RadioItem>
>(function DropdownMenuRadioItem({ className, children, ...props }, ref) {
  return (
    <Menu.RadioItem ref={ref} className={clsx(MENU_ITEM, className)} {...props}>
      <span className={MENU_INDICATOR}>
        <Menu.ItemIndicator>
          <Check className={ICON.small} aria-hidden="true" />
        </Menu.ItemIndicator>
      </span>
      {children}
    </Menu.RadioItem>
  );
});

export const DropdownMenuLabel = forwardRef<
  ElementRef<typeof Menu.Label>,
  ComponentPropsWithoutRef<typeof Menu.Label>
>(function DropdownMenuLabel({ className, ...props }, ref) {
  return <Menu.Label ref={ref} className={clsx(MENU_LABEL, className)} {...props} />;
});

export const DropdownMenuSeparator = forwardRef<
  ElementRef<typeof Menu.Separator>,
  ComponentPropsWithoutRef<typeof Menu.Separator>
>(function DropdownMenuSeparator({ className, ...props }, ref) {
  return <Menu.Separator ref={ref} className={clsx(MENU_SEPARATOR, className)} {...props} />;
});

/** Trailing text in a menu item: a shortcut in Apple glyph order (⇧⌘C) or a count. */
export function MenuTrailing({ children }: { children: React.ReactNode }) {
  return <span className={MENU_TRAILING}>{children}</span>;
}
