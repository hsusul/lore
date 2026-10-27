import { clsx } from "clsx";
import { Children, forwardRef, Fragment, type ComponentPropsWithoutRef, type ReactNode } from "react";

/**
 * System Settings' grouped inset form (spec 9.11): a `--fill-3` section with 10px corners,
 * rows of label left and control right, and a hairline between rows inset 12px.
 */
export function FormGroup({ label, children, className }: { label?: string; children: ReactNode; className?: string }) {
  const rows = Children.toArray(children);
  return (
    <div role="group" aria-label={label} className={clsx("rounded-group bg-fill-3 shadow-ring", className)}>
      {rows.map((row, index) => (
        <Fragment key={index}>
          {index > 0 && <div aria-hidden="true" className="mx-3 h-ring bg-separator" />}
          {row}
        </Fragment>
      ))}
    </div>
  );
}

/** One form row: the label (and an optional description under it) left, the control right. */
export function FormRow({
  label,
  description,
  labelFor,
  children,
}: {
  label: ReactNode;
  description?: ReactNode;
  labelFor?: string;
  children?: ReactNode;
}) {
  const Label = labelFor ? "label" : "span";
  return (
    <div className="flex min-h-form-row items-center gap-3 px-3 py-2">
      <div className="min-w-0 flex-1">
        <Label htmlFor={labelFor} className="block text-body text-label">
          {label}
        </Label>
        {description && <div className="mt-0.5 text-callout text-label-2">{description}</div>}
      </div>
      {children}
    </div>
  );
}

/** A section header above a group: Subheadline semibold in secondary text. */
export function GroupHeader({ children }: { children: ReactNode }) {
  return <h3 className="mb-1.5 ml-3 mt-5 text-subheadline font-semibold text-label-2">{children}</h3>;
}

/** A macOS text field: 22px, a hairline bezel, and the focus ring while it has focus. */
export const TextField = forwardRef<HTMLInputElement, ComponentPropsWithoutRef<"input">>(function TextField(
  { className, ...props },
  ref,
) {
  return (
    <input
      ref={ref}
      className={clsx(
        "h-control min-w-0 rounded-control bg-bg-content px-1.5 text-control text-label shadow-ring outline-none border-contrast border-separator focus:shadow-focus disabled:text-label-3",
        className,
      )}
      {...props}
    />
  );
});

/** Help text under a group, in sentences. */
export function Footnote({ children, role }: { children: ReactNode; role?: "status" | "alert" }) {
  return (
    <p role={role} className="mx-3 mt-1.5 text-callout text-label-2 select-text">
      {children}
    </p>
  );
}
