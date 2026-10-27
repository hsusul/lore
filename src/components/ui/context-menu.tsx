import * as Menu from "@radix-ui/react-context-menu";
import { clsx } from "clsx";
import { ChevronRight } from "lucide-react";
import { forwardRef, type ComponentPropsWithoutRef, type ElementRef } from "react";

import { ICON } from "./button";
import { MENU_ITEM, MENU_SEPARATOR, MENU_SURFACE } from "./menu-styles";

/** Right-click menus, styled as macOS menus (spec 9.12, 16.2). Every item also lives in the palette. */
export const ContextMenu = Menu.Root;
export const ContextMenuTrigger = Menu.Trigger;
export const ContextMenuSub = Menu.Sub;

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

/** An item that opens a submenu, with the trailing chevron macOS menus draw. */
export const ContextMenuSubTrigger = forwardRef<
  ElementRef<typeof Menu.SubTrigger>,
  ComponentPropsWithoutRef<typeof Menu.SubTrigger>
>(function ContextMenuSubTrigger({ className, children, ...props }, ref) {
  return (
    <Menu.SubTrigger ref={ref} className={clsx(MENU_ITEM, "data-open:bg-fill", className)} {...props}>
      {children}
      <ChevronRight className={clsx(ICON.small, "ml-auto")} aria-hidden="true" />
    </Menu.SubTrigger>
  );
});

export const ContextMenuSubContent = forwardRef<
  ElementRef<typeof Menu.SubContent>,
  ComponentPropsWithoutRef<typeof Menu.SubContent>
>(function ContextMenuSubContent({ className, ...props }, ref) {
  return (
    <Menu.Portal>
      <Menu.SubContent ref={ref} className={clsx(MENU_SURFACE, className)} {...props} />
    </Menu.Portal>
  );
});
