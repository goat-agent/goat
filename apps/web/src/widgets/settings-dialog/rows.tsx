import type { ReactNode } from "react";

export function Heading({ title, action }: { title: string; action?: ReactNode }) {
  return (
    <div className="flex min-h-8 items-center justify-between">
      <h3 className="text-xs font-medium text-muted-foreground">{title}</h3>
      {action}
    </div>
  );
}

export function List({ children }: { children: ReactNode }) {
  return <ul className="divide-y rounded-xl border">{children}</ul>;
}

export function Row({
  icon,
  title,
  detail,
  isWarning,
  isIndented,
  children,
}: {
  icon?: ReactNode;
  title: string;
  detail?: string;
  isWarning?: boolean;
  isIndented?: boolean;
  children?: ReactNode;
}) {
  return (
    <li
      className={`flex items-center gap-3 px-3 py-2.5 ${isIndented ? "min-h-12 ps-12" : "min-h-14"}`}
    >
      {icon}
      <div className="min-w-0 flex-1">
        <p className={`truncate ${isIndented ? "" : "font-medium"}`}>{title}</p>
        {detail && (
          <p className={`text-xs ${isWarning ? "text-destructive" : "text-muted-foreground"}`}>
            {detail}
          </p>
        )}
      </div>
      {children}
    </li>
  );
}
