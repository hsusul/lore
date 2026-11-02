import * as Menu from "@radix-ui/react-context-menu";
import { clsx } from "clsx";
import { forwardRef, type ComponentPropsWithoutRef, type ElementRef } from "react";

import { MENU_ITEM, MENU_SEPARATOR, MENU_SURFACE } from "./menu-styles";

/** Right-click menus, styled as macOS menus (spec 9.12, 16.2). Every item also lives in the palette. */
export const ContextMenu = Menu.Root;
export const ContextMenuTrigger = Menu.Trigger;

export const ContextMenuContent = forwardRef<
  ElementRef<typeof Menu.Content>,
  ComponentPropsWithoutRef<typeof Menu.Content>
>(function ContextMenuContent({ className, ...props }, ref) {
  return (
    <Menu.Portal>
      <Menu.Content ref={ref} className={clsx(MENU_SURFACE, className)} {...props} />
    </Menu.Portal>
  );
});

export const ContextMenuItem = forwardRef<
  ElementRef<typeof Menu.Item>,
  ComponentPropsWithoutRef<typeof Menu.Item>
>(function ContextMenuItem({ className, ...props }, ref) {
  return <Menu.Item ref={ref} className={clsx(MENU_ITEM, className)} {...props} />;
});

export const ContextMenuSeparator = forwardRef<
  ElementRef<typeof Menu.Separator>,
  ComponentPropsWithoutRef<typeof Menu.Separator>
>(function ContextMenuSeparator({ className, ...props }, ref) {
  return <Menu.Separator ref={ref} className={clsx(MENU_SEPARATOR, className)} {...props} />;
});
