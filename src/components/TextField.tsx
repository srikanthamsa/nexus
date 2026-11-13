import { useId, type InputHTMLAttributes, type ReactNode, type TextareaHTMLAttributes } from "react";
import { cn } from "../lib/utils";

interface TextFieldProps extends InputHTMLAttributes<HTMLInputElement> {
  label?: string;
  hint?: string;
  error?: string;
  icon?: ReactNode;
  suffix?: ReactNode;
  containerClassName?: string;
}

export function TextField({
  label,
  hint,
  error,
  icon,
  suffix,
  className,
  containerClassName,
  id,
  ...rest
}: TextFieldProps) {
  const generatedId = useId();
  const inputId = id ?? generatedId;
  const describedBy = error ? `${inputId}-error` : hint ? `${inputId}-hint` : undefined;

  return (
    <div className={cn("field", containerClassName)}>
      {label && (
        <label className="field__label" htmlFor={inputId}>
          {label}
        </label>
      )}
      <div className={cn("field__control", error && "has-error")}>
        {icon && <span className="field__icon">{icon}</span>}
        <input
          id={inputId}
          className={cn("field__input", icon ? "has-icon" : undefined, className)}
          aria-invalid={Boolean(error)}
          aria-describedby={describedBy}
          {...rest}
        />
        {suffix && <span className="field__suffix">{suffix}</span>}
      </div>
      {error ? (
        <p className="field__hint field__hint--error" id={`${inputId}-error`}>
          {error}
        </p>
      ) : (
        hint && (
          <p className="field__hint" id={`${inputId}-hint`}>
            {hint}
          </p>
        )
      )}
    </div>
  );
}

interface TextAreaFieldProps extends TextareaHTMLAttributes<HTMLTextAreaElement> {
  label?: string;
  hint?: string;
}

export function TextAreaField({ label, hint, className, id, ...rest }: TextAreaFieldProps) {
  const generatedId = useId();
  const areaId = id ?? generatedId;
  return (
    <div className="field">
      {label && (
        <label className="field__label" htmlFor={areaId}>
          {label}
        </label>
      )}
      <textarea
        id={areaId}
        className={cn("field__input field__input--area", className)}
        {...rest}
      />
      {hint && <p className="field__hint">{hint}</p>}
    </div>
  );
}
