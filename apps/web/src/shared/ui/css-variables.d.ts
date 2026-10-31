import "react";

declare module "react" {
  // Lets `style` set CSS custom properties, such as `--sidebar-width`, without a type assertion.
  interface CSSProperties {
    [variable: `--${string}`]: string | number | undefined;
  }
}
