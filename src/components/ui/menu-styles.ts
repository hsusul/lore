/**
 * macOS menu anatomy (spec 9.12), shared by DropdownMenu, ContextMenu and Select:
 * a 10px-radius surface with the menu elevation, 22px items, the active item filled
 * with the selection color, shortcuts and counts trailing in secondary text.
 */
export const MENU_SURFACE =
  "z-50 min-w-menu overflow-hidden rounded-group bg-bg-popover p-1.5 text-control text-label shadow-menu outline-none select-none data-open:animate-popover-in data-closed:animate-popover-out";

export const MENU_ITEM =
  "group relative flex h-menu-row items-center gap-1.5 rounded-control pl-6 pr-2 whitespace-nowrap outline-none data-highlighted:bg-selected data-highlighted:text-on-selected data-disabled:text-label-3";

export const MENU_INDICATOR = "absolute left-1.5 inline-flex items-center";

export const MENU_TRAILING =
  "ml-auto pl-4 text-subheadline tabular-nums text-label-2 group-data-highlighted:text-on-selected";

export const MENU_LABEL = "px-2 pb-1 pt-1.5 text-subheadline font-semibold text-label-2";

export const MENU_SEPARATOR = "mx-2 my-1.5 h-px bg-separator";
