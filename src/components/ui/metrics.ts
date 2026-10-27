/**
 * The few spec values Radix takes as JavaScript numbers rather than CSS. Everything
 * else is a token in tokens.css.
 */

/** Tooltips appear after 600ms (spec 9.12). */
export const TOOLTIP_DELAY_MS = 600;

/** Tooltips and popovers sit 8px from their control (spec 18.11). */
export const FLOAT_OFFSET = 8;

/** Menus open flush to their trigger, as macOS pull-down menus do. */
export const MENU_OFFSET = 4;

/** How long a control's confirmation ("Copied") holds its tooltip open. */
export const CONFIRM_MS = 1500;
