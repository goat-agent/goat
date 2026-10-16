import type { Frame, Size } from "@goat/terminal";
import { type Composer, renderComposer } from "./composer.ts";
import type { Line, Renderer } from "./render.ts";
import type { Block } from "./transcript.ts";

export interface View {
  readonly blocks: readonly Block[];
  readonly composer: Composer;
  readonly top: number | undefined;
  readonly hint: string;
}

export interface Placement {
  readonly top: number;
  readonly height: number;
  readonly total: number;
}

export function layout(
  view: View,
  size: Size,
  renderer: Renderer,
): { readonly frame: Frame; readonly placement: Placement } {
  const composer = renderComposer(
    view.composer,
    size.columns,
    Math.max(1, Math.floor(size.rows / 3)),
    view.hint,
  );
  const height = Math.max(0, size.rows - composer.lines.length);
  const rendered = view.blocks
    .map((block) => renderer.render(block, size.columns))
    .filter((lines) => lines.length > 0);
  const total =
    rendered.reduce((sum, lines) => sum + lines.length, 0) + Math.max(0, rendered.length - 1);
  const top = Math.min(Math.max(0, view.top ?? total - height), Math.max(0, total - height));
  const lines = visible(rendered, top, height);
  return {
    frame: {
      lines: [
        ...lines.map((line) => line.text),
        ...Array.from({ length: height - lines.length }, () => ""),
        ...composer.lines,
      ],
      cursor: { row: height + composer.cursor.row, column: composer.cursor.column },
    },
    placement: { top, height, total },
  };
}

const blank: Line = { text: "" };

function visible(
  rendered: readonly (readonly Line[])[],
  top: number,
  height: number,
): readonly Line[] {
  const lines: Line[] = [];
  let offset = 0;
  for (const [index, block] of rendered.entries()) {
    const withGap = index === 0 ? block : [blank, ...block];
    if (offset + withGap.length > top) {
      lines.push(...withGap.slice(Math.max(0, top - offset)));
      if (lines.length >= height) {
        break;
      }
    }
    offset += withGap.length;
  }
  return lines.slice(0, height);
}
