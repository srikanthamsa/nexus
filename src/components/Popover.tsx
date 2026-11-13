import { useEffect, useRef, useState, type ReactNode } from "react";
import { cn } from "../lib/utils";

type PopoverChildren = ReactNode | ((close: () => void) => ReactNode);

interface PopoverProps {
  button: ReactNode;
  ariaLabel: string;
  children: PopoverChildren;
  align?: "start" | "end";
  width?: number;
  buttonClassName?: string;
  panelClassName?: string;
  role?: string;
}

export function Popover({
  button,
  ariaLabel,
  children,
  align = "end",
  width,
  buttonClassName,
  panelClassName,
  role = "dialog",
}: PopoverProps) {
  const [open, setOpen] = useState(false);
  const wrapperRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;
    const handlePointerDown = (event: MouseEvent) => {
      if (!wrapperRef.current?.contains(event.target as Node)) setOpen(false);
    };
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") setOpen(false);
    };
    document.addEventListener("mousedown", handlePointerDown);
    document.addEventListener("keydown", handleKeyDown);
    return () => {
      document.removeEventListener("mousedown", handlePointerDown);
      document.removeEventListener("keydown", handleKeyDown);
    };
  }, [open]);

  const close = () => setOpen(false);

  return (
    <div className="popover" ref={wrapperRef}>
      <button
        type="button"
        className={cn("popover__trigger", buttonClassName)}
        aria-label={ariaLabel}
        aria-haspopup={role === "menu" ? "menu" : "dialog"}
        aria-expanded={open}
        onClick={() => setOpen((value) => !value)}
      >
        {button}
      </button>
      {open && (
        <div
          className={cn("popover__panel", `popover__panel--${align}`, panelClassName)}
          role={role}
          style={width ? { width } : undefined}
        >
          {typeof children === "function" ? children(close) : children}
        </div>
      )}
    </div>
  );
}

export interface MenuItem {
  label: string;
  icon?: ReactNode;
  onClick: () => void;
  danger?: boolean;
  disabled?: boolean;
  dividerBefore?: boolean;
}

export function Menu({
  button,
  ariaLabel,
  items,
  align = "end",
  width = 220,
  buttonClassName,
}: {
  button: ReactNode;
  ariaLabel: string;
  items: Array<MenuItem | null>;
  align?: "start" | "end";
  width?: number;
  buttonClassName?: string;
}) {
  return (
    <Popover
      button={button}
      ariaLabel={ariaLabel}
      align={align}
      width={width}
      buttonClassName={buttonClassName}
      role="menu"
    >
      {(close) => (
        <div className="menu">
          {items.filter(Boolean).map((item, index) => {
            if (!item) return null;
            return (
              <div key={`${item.label}-${index}`}>
                {item.dividerBefore && <div className="menu__divider" />}
                <button
                  type="button"
                  role="menuitem"
                  className={cn("menu__item", item.danger && "is-danger")}
                  disabled={item.disabled}
                  onClick={() => {
                    if (item.disabled) return;
                    close();
                    item.onClick();
                  }}
                >
                  {item.icon && <span className="menu__icon">{item.icon}</span>}
                  <span>{item.label}</span>
                </button>
              </div>
            );
          })}
        </div>
      )}
    </Popover>
  );
}
