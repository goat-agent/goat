import { m } from "@/shared/i18n/paraglide/messages";
import { isTheme, setTheme, useTheme } from "@/shared/lib/theme";
import { ToggleGroup, ToggleGroupItem } from "@/shared/ui/toggle-group";

/** Settings that belong to this browser rather than the account. */
export function General() {
  const theme = useTheme();
  return (
    <div className="flex flex-col gap-2">
      <p className="text-sm font-medium">{m.theme()}</p>
      <ToggleGroup
        type="single"
        variant="outline"
        value={theme}
        onValueChange={(value: string) => {
          if (isTheme(value)) setTheme(value);
        }}
        aria-label={m.theme()}
        className="self-start"
      >
        <ToggleGroupItem value="system">{m.theme_system()}</ToggleGroupItem>
        <ToggleGroupItem value="light">{m.theme_light()}</ToggleGroupItem>
        <ToggleGroupItem value="dark">{m.theme_dark()}</ToggleGroupItem>
      </ToggleGroup>
    </div>
  );
}
