import { z } from "@hono/zod-openapi";
import { openEnum } from "../openapi";

// What the server and a computer say to each other through `devices.call`. A window ref is the
// computer's own handle; the server turns it into the short id the model sees.

export const methods = {
  status: "computer.status",
  windows: "computer.windows",
  view: "computer.view",
  click: "computer.click",
  drag: "computer.drag",
  scroll: "computer.scroll",
  move: "computer.move",
  type: "computer.type",
  key: "computer.key",
  open: "computer.open",
} as const;

export const ComputerStatus = z
  .object({
    isAvailable: z
      .boolean()
      .describe("The user lets the agent use this computer and granted the permissions it needs"),
  })
  .openapi("ComputerStatus");

const ComputerWindow = z
  .object({
    ref: z.string(),
    app: z.string(),
    title: z.string(),
    isFront: z.boolean(),
    isMinimized: z.boolean(),
  })
  .openapi("ComputerWindow");

export const ComputerWindows = z
  .object({ windows: z.array(ComputerWindow).describe("Front to back") })
  .openapi("ComputerWindows");

const ComputerImage = z
  .object({
    data: z.string().describe("Base64 JPEG"),
    width: z.int().positive(),
    height: z.int().positive(),
  })
  .openapi("ComputerImage");

export const ComputerView = z
  .object({ window: ComputerWindow, image: ComputerImage })
  .openapi("ComputerView");

const ComputerPoint = z
  .object({ x: z.number(), y: z.number() })
  .describe("Pixels of the window's latest full image, not of a zoom")
  .openapi("ComputerPoint");

export const ComputerOutcome = z
  .object({
    delivery: openEnum(["background", "foreground"])
      .optional()
      .describe("How the action reached the app; absent when it was not done"),
    notDone: openEnum(["needs_foreground", "declined", "minimized"])
      .optional()
      .describe(
        "Why it was not done: the app takes it only in front, the user did not allow bringing it forward, or the window is minimized",
      ),
    view: ComputerView.optional(),
  })
  .openapi("ComputerOutcome");

export type ComputerOutcome = z.infer<typeof ComputerOutcome>;

const window = z.string().describe("A window ref from computer.windows");
const foreground = z
  .boolean()
  .optional()
  .describe("Bring the app to the front for this action; the computer asks the user first");
const modifiers = z.array(openEnum(["cmd", "shift", "option", "ctrl"])).optional();

export const params = {
  status: z.object({}).openapi("ComputerStatusParams"),
  windows: z.object({}).openapi("ComputerWindowsParams"),
  view: z
    .object({
      window,
      region: z
        .object({ x: z.number(), y: z.number(), width: z.number(), height: z.number() })
        .optional()
        .describe("Zoom: show this part of the window's last image at full resolution"),
      waitSeconds: z.number().optional().describe("Wait this long before looking"),
    })
    .openapi("ComputerViewParams"),
  click: z
    .object({
      window,
      at: ComputerPoint,
      button: openEnum(["left", "right", "middle"]).optional(),
      count: z.int().min(1).max(3).optional(),
      modifiers,
      foreground,
    })
    .openapi("ComputerClickParams"),
  drag: z
    .object({ window, from: ComputerPoint, to: ComputerPoint, modifiers, foreground })
    .openapi("ComputerDragParams"),
  scroll: z
    .object({
      window,
      at: ComputerPoint,
      direction: openEnum(["up", "down", "left", "right"]),
      amount: z.int().min(1).describe("Wheel notches"),
      foreground,
    })
    .openapi("ComputerScrollParams"),
  move: z.object({ window, at: ComputerPoint, foreground }).openapi("ComputerMoveParams"),
  type: z.object({ window, text: z.string(), foreground }).openapi("ComputerTypeParams"),
  key: z
    .object({
      window,
      keys: z.string().describe("Keys joined with +, such as cmd+shift+t or return"),
      repeat: z.int().min(1).optional(),
      foreground,
    })
    .openapi("ComputerKeyParams"),
  open: z
    .object({
      app: z.string().optional(),
      path: z.string().optional(),
      url: z.string().optional(),
      foreground,
    })
    .openapi("ComputerOpenParams"),
};

export type ActionMethod = "click" | "drag" | "scroll" | "move" | "type" | "key";

/** Registered in the OpenAPI document, keyed by name, so the apps get these types generated. */
export const schemas = {
  ComputerStatus,
  ComputerWindows,
  ComputerView,
  ComputerOutcome,
  ComputerStatusParams: params.status,
  ComputerWindowsParams: params.windows,
  ComputerViewParams: params.view,
  ComputerClickParams: params.click,
  ComputerDragParams: params.drag,
  ComputerScrollParams: params.scroll,
  ComputerMoveParams: params.move,
  ComputerTypeParams: params.type,
  ComputerKeyParams: params.key,
  ComputerOpenParams: params.open,
};
