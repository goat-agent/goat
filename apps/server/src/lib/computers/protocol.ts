import { z } from "@hono/zod-openapi";
import { openEnum } from "../openapi";

// What the server and a computer say to each other through `devices.call`. Refs are the
// computer's own handles for windows and elements; the server turns them into the short ids the
// model sees, so the computer never needs to know about other computers.

export const methods = {
  status: "computer.status",
  screen: "computer.screen",
  view: "computer.view",
  click: "computer.click",
  hover: "computer.hover",
  drag: "computer.drag",
  scroll: "computer.scroll",
  type: "computer.type",
  key: "computer.key",
  set: "computer.set",
  menu: "computer.menu",
  open: "computer.open",
  clipboard: "computer.clipboard",
} as const;

export const ComputerStatus = z
  .object({
    isAvailable: z
      .boolean()
      .describe("The user lets the agent use this computer and granted the permissions it needs"),
  })
  .openapi("ComputerStatus");

const ComputerImage = z
  .object({
    data: z.string().describe("Base64 JPEG"),
    width: z.int().positive(),
    height: z.int().positive(),
  })
  .describe("Points in targets are pixels of the window's latest full image, not of a zoom")
  .openapi("ComputerImage");

const ComputerWindow = z
  .object({ ref: z.string(), app: z.string(), title: z.string() })
  .openapi("ComputerWindow");

const ComputerScreenWindow = z
  .object({
    ref: z.string(),
    app: z.string(),
    title: z.string(),
    isFront: z.boolean(),
    isMinimized: z.boolean(),
  })
  .openapi("ComputerScreenWindow");

export const ComputerScreen = z
  .object({
    ref: z
      .string()
      .describe("Acts as a window covering the whole screen, for points outside windows"),
    image: ComputerImage.optional(),
    windows: z.array(ComputerScreenWindow).describe("Front to back"),
  })
  .openapi("ComputerScreen");

const ComputerNode = z
  .object({
    ref: z.string().optional().describe("Present when the element can be acted on"),
    depth: z.int().min(0),
    role: z.string().describe("Lowercase words, such as button or text field"),
    label: z.string().optional(),
    value: z.string().optional(),
    states: z.array(z.string()).optional().describe("Such as selected, focused, disabled"),
  })
  .openapi("ComputerNode");

export const ComputerView = z
  .object({
    window: ComputerWindow,
    image: ComputerImage,
    nodes: z.array(ComputerNode).describe("The window's elements in tree order"),
    isTruncated: z.boolean().optional().describe("Elements were left out to keep the view small"),
  })
  .openapi("ComputerView");

export const ComputerOutcome = z
  .object({
    view: ComputerView.optional(),
    isStale: z
      .boolean()
      .optional()
      .describe("The target element no longer matched; nothing was done and view shows the window"),
    clipboard: z
      .object({ text: z.string().optional(), files: z.array(z.string()).optional() })
      .optional()
      .describe("What the clipboard holds, when it was read"),
  })
  .openapi("ComputerOutcome");

export const ComputerTarget = z
  .object({
    element: z.string().optional(),
    window: z.string().optional(),
    x: z.number().optional(),
    y: z.number().optional(),
  })
  .describe("An element, a whole window, or a point on a window's image")
  .openapi("ComputerTarget");

export type ComputerTarget = z.infer<typeof ComputerTarget>;

const modifiers = z.array(openEnum(["cmd", "shift", "option", "ctrl"])).optional();

export const params = {
  status: z.object({}).openapi("ComputerStatusParams"),
  screen: z.object({ includeImage: z.boolean() }).openapi("ComputerScreenParams"),
  view: z
    .object({
      window: z.string(),
      region: z
        .object({ x: z.number(), y: z.number(), width: z.number(), height: z.number() })
        .optional()
        .describe("Zoom: show this part of the window's last image at full resolution"),
      waitSeconds: z.number().optional().describe("Wait this long before looking"),
    })
    .openapi("ComputerViewParams"),
  click: z
    .object({
      target: ComputerTarget,
      button: openEnum(["left", "right", "middle"]).optional(),
      count: z.int().min(1).max(3).optional(),
      modifiers,
    })
    .openapi("ComputerClickParams"),
  hover: z.object({ target: ComputerTarget }).openapi("ComputerHoverParams"),
  drag: z
    .object({ from: ComputerTarget, to: ComputerTarget, modifiers })
    .openapi("ComputerDragParams"),
  scroll: z
    .object({
      target: ComputerTarget,
      direction: openEnum(["up", "down", "left", "right"]),
      amount: z.int().min(1),
    })
    .openapi("ComputerScrollParams"),
  type: z.object({ target: ComputerTarget, text: z.string() }).openapi("ComputerTypeParams"),
  key: z
    .object({ target: ComputerTarget, keys: z.string(), repeat: z.int().min(1).optional() })
    .openapi("ComputerKeyParams"),
  set: z.object({ target: ComputerTarget, value: z.string() }).openapi("ComputerSetParams"),
  menu: z
    .object({ window: z.string(), path: z.array(z.string()).min(1) })
    .openapi("ComputerMenuParams"),
  open: z
    .object({ app: z.string().optional(), path: z.string().optional(), url: z.string().optional() })
    .openapi("ComputerOpenParams"),
  clipboard: z
    .object({ text: z.string().optional(), path: z.string().optional() })
    .describe("Writes text or a file when one is given; reads otherwise")
    .openapi("ComputerClipboardParams"),
};

/** Registered in the OpenAPI document, keyed by name, so the apps get these types generated. */
export const schemas = {
  ComputerStatus,
  ComputerScreen,
  ComputerView,
  ComputerOutcome,
  ComputerStatusParams: params.status,
  ComputerScreenParams: params.screen,
  ComputerViewParams: params.view,
  ComputerClickParams: params.click,
  ComputerHoverParams: params.hover,
  ComputerDragParams: params.drag,
  ComputerScrollParams: params.scroll,
  ComputerTypeParams: params.type,
  ComputerKeyParams: params.key,
  ComputerSetParams: params.set,
  ComputerMenuParams: params.menu,
  ComputerOpenParams: params.open,
  ComputerClipboardParams: params.clipboard,
};
