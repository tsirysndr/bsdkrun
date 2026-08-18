import { Input, Kbd } from "@heroui/react";
import { useAtom } from "jotai";
import { IconSearch } from "@tabler/icons-react";
import type { ReactNode } from "react";
import { filterAtom } from "../state/atoms";

export function ViewShell({
  title,
  subtitle,
  actions,
  searchPlaceholder,
  children,
}: {
  title: string;
  subtitle?: string;
  actions?: ReactNode;
  searchPlaceholder?: string;
  children: ReactNode;
}) {
  const [filter, setFilter] = useAtom(filterAtom);
  return (
    <div className="flex h-full min-h-0 flex-col">
      <div className="flex items-center gap-3 px-6 pb-3 pt-5">
        <div className="min-w-0">
          <h1 className="text-lg font-semibold tracking-tight">{title}</h1>
          {subtitle && (
            <p className="mt-0.5 text-xs text-foreground-500">{subtitle}</p>
          )}
        </div>
        <div className="flex-1" />
        <Input
          data-filter-input
          size="sm"
          radius="lg"
          value={filter}
          onValueChange={setFilter}
          placeholder={searchPlaceholder || "Filter…"}
          endContent={
            <Kbd className="bg-default-100 text-foreground-500">f</Kbd>
          }
          startContent={<IconSearch size={16} className="text-foreground-400" />}
          className="max-w-64"
          classNames={{ inputWrapper: "bg-content2/60 border border-white/10" }}
        />
        {actions}
      </div>
      <div className="min-h-0 flex-1 overflow-auto px-6 pb-6">{children}</div>
    </div>
  );
}

export function EmptyState({
  icon,
  title,
  hint,
  action,
}: {
  icon: ReactNode;
  title: string;
  hint?: string;
  action?: ReactNode;
}) {
  return (
    <div className="grid h-full place-items-center">
      <div className="flex max-w-sm flex-col items-center text-center">
        <div className="mb-4 grid h-16 w-16 place-items-center rounded-2xl border border-white/10 bg-content2/50 text-foreground-400">
          {icon}
        </div>
        <div className="text-base font-medium text-foreground-400">{title}</div>
        {hint && (
          <p className="mt-1.5 text-sm text-foreground-500">{hint}</p>
        )}
        {action && <div className="mt-5">{action}</div>}
      </div>
    </div>
  );
}
