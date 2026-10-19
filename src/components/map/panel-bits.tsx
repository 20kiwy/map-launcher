"use client";

import type { ReactNode } from "react";
import { AlertTriangle, RefreshCw } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/skeleton";

/** Actionable error state with a retry button (tile server may be down). */
export function ErrorNote({
  title,
  detail,
  onRetry,
}: {
  title: string;
  detail?: string | null;
  onRetry: () => void;
}) {
  return (
    <div
      role="alert"
      className="flex flex-col gap-2 rounded-md border border-[#dc2a67]/30 bg-[#dc2a67]/5 p-3"
    >
      <div className="flex items-start gap-2 text-[#8f1f48]">
        <AlertTriangle className="mt-0.5 size-4 shrink-0" aria-hidden="true" />
        <div className="text-xs">
          <p className="font-medium">{title}</p>
          {detail ? <p className="mt-0.5 text-[11px] opacity-80">{detail}</p> : null}
        </div>
      </div>
      <Button
        type="button"
        variant="outline"
        size="sm"
        className="min-h-9 self-start border-[#dc2a67]/40 text-[#8f1f48] hover:bg-[#dc2a67]/10"
        onClick={onRetry}
      >
        <RefreshCw className="size-3.5" aria-hidden="true" />
        Retry
      </Button>
    </div>
  );
}

/** Skeleton rows used while registries load. */
export function ListSkeleton({ rows = 6 }: { rows?: number }) {
  return (
    <div className="flex flex-col gap-2" aria-hidden="true">
      {Array.from({ length: rows }, (_, i) => (
        <Skeleton key={i} className="h-11 w-full" />
      ))}
    </div>
  );
}

/** Small muted empty-state note. */
export function EmptyNote({ children }: { children: ReactNode }) {
  return (
    <p className="rounded-md border border-dashed border-[#2c2c2c]/15 bg-white/40 px-3 py-4 text-center text-xs text-[#6b6b66]">
      {children}
    </p>
  );
}

/** Count line shown above registry lists. */
export function CountLine({ children }: { children: ReactNode }) {
  return (
    <p className="flex items-center justify-between gap-2 text-[11px] text-[#6b6b66]">
      {children}
    </p>
  );
}
