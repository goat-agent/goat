import type { ReactNode } from "react";
import { Dialog, DialogContent, DialogTitle } from "@/shared/ui/dialog";

const SIZES = {
  // Grows with its content, up to the window.
  form: "sm:h-auto sm:max-h-[min(47.5rem,calc(100dvh-3rem))] sm:max-w-[35rem]",
  // Keeps one height while its sections change, so the window does not jump.
  panel: "sm:h-[min(40rem,calc(100dvh-3rem))] sm:max-w-[47.5rem]",
};

/**
 * A dialog over the page that fills the screen when the window is compact, the way a sheet does
 * on a phone. `leading` sits before the title, such as a back button.
 */
export function ResponsiveDialog({
  title,
  leading,
  size,
  onClose,
  children,
}: {
  title: string;
  leading?: ReactNode;
  size: keyof typeof SIZES;
  onClose: () => void;
  children: ReactNode;
}) {
  return (
    <Dialog open onOpenChange={(open) => !open && onClose()}>
      <DialogContent
        className={`flex h-dvh max-w-none flex-col gap-0 overflow-hidden rounded-none p-0 sm:rounded-2xl ${SIZES[size]}`}
      >
        <div className="flex h-14 shrink-0 items-center gap-1 border-b ps-5 pe-12">
          {leading}
          <DialogTitle className="truncate text-[15px]">{title}</DialogTitle>
        </div>
        {children}
      </DialogContent>
    </Dialog>
  );
}
