import { CaretDown as ChevronDown } from "@phosphor-icons/react";
import type { SelectHTMLAttributes } from "react";
import { cn } from "../lib/utils";

interface SelectProps extends SelectHTMLAttributes<HTMLSelectElement> {
  label?: string;
  hint?: string;
  options: Array<{ value: string; label: string }>;
  compact?: boolean;
}

export function Select({ label, hint, options, compact, className, id, ...rest }: SelectProps) {
  return (
    <div className={cn("field", compact && "field--compact")}>
      {label && (
        <label className="field__label" htmlFor={id}>
          {label}
        </label>
      )}
      <div className="select">
        <select id={id} className={cn("select__input", className)} {...rest}>
          {options.map((option) => (
            <option value={option.value} key={option.value}>
              {option.label}
            </option>
          ))}
        </select>
        <ChevronDown size={14} className="select__chevron" aria-hidden="true" />
      </div>
      {hint && <p className="field__hint">{hint}</p>}
    </div>
  );
}
