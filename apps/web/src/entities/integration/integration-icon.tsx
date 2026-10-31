import { cn } from "cn";

export function IntegrationIcon({ src, className }: { src: string; className?: string }) {
  return (
    <img
      src={src}
      alt=""
      className={cn("size-6 shrink-0 rounded-md bg-white ring-1 ring-border", className)}
    />
  );
}
