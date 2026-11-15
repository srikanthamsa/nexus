import type { ButtonHTMLAttributes, ReactNode } from "react";
import { CircleNotch as Loader2 } from "@phosphor-icons/react";
import { cn } from "../lib/utils";

type ButtonVariant = "primary" | "secondary" | "ghost" | "danger" | "outline";
type ButtonSize = "sm" | "md" | "lg";

interface ButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: ButtonVariant;
  size?: ButtonSize;
  leftIcon?: ReactNode;
  rightIcon?: ReactNode;
  loading?: boolean;
  block?: boolean;
}

export function Button({
  variant = "secondary",
  size = "md",
  leftIcon,
  rightIcon,
  loading = false,
  block = false,
  className,
  children,
  disabled,
  type = "button",
  ...rest
}: ButtonProps) {
  return (
    <button
      type={type}
      className={cn(
        "btn",
        `btn--${variant}`,
        `btn--${size}`,
        block && "btn--block",
        loading && "is-loading",
        className,
      )}
      disabled={disabled || loading}
      {...rest}
    >
      {loading ? <Loader2 size={15} className="spin" /> : leftIcon}
      {children !== undefined && children !== null && <span>{children}</span>}
      {rightIcon}
    </button>
  );
}

interface IconButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  label: string;
  icon: ReactNode;
  size?: "sm" | "md" | "lg";
  tone?: "default" | "danger";
  active?: boolean;
}

export function IconButton({
  label,
  icon,
  size = "md",
  tone = "default",
  active = false,
  className,
  type = "button",
  ...rest
}: IconButtonProps) {
  return (
    <button
      type={type}
      className={cn(
        "icon-btn",
        `icon-btn--${size}`,
        tone === "danger" && "icon-btn--danger",
        active && "is-active",
        className,
      )}
      aria-label={label}
      title={label}
      {...rest}
    >
      {icon}
    </button>
  );
}
