import { z } from "zod";

/**
 * Everything the web app and the native app showing it say to each other. These schemas are the
 * only definition: `bun run bridge` exports them, and the Mac app generates its Swift types from
 * the export, as the apps do from the server's OpenAPI document.
 */
export const contract = z.registry<{ id: string }>();

function named<T extends z.ZodType>(id: string, schema: T): T {
  contract.add(schema, { id });
  return schema;
}

const Empty = named("Empty", z.object({}));

/** Put on the page by the native app before any script runs. */
export const Host = named(
  "Host",
  z.object({
    platform: z.enum(["macos"]),
    /** This install as the server knows it; sign-in sends it so the server can reach this Mac. */
    device: named("HostDevice", z.object({ installationId: z.uuid(), name: z.string() })),
  }),
);

const Permission = named("Permission", z.enum(["accessibility", "screenRecording"]));

const ComputerUse = named(
  "ComputerUse",
  z.object({
    isEnabled: z.boolean(),
    permissions: named(
      "ComputerUsePermissions",
      z.object({ accessibility: z.boolean(), screenRecording: z.boolean() }),
    ),
  }),
);

export const RequestMethod = named(
  "RequestMethod",
  z.enum([
    "moveWindow",
    "doubleClickTitleBar",
    "authenticate",
    "computerUse",
    "setComputerUse",
    "openPermissionSettings",
  ]),
);

/** Calls the page makes and the native app answers. */
export const requests = {
  moveWindow: { params: Empty, result: Empty },
  doubleClickTitleBar: { params: Empty, result: Empty },
  /** Opens a service's sign-in apart from the page; no callback means the user closed it. */
  authenticate: {
    params: named("AuthenticateParams", z.object({ url: z.url() })),
    result: named("AuthenticateResult", z.object({ callbackUrl: z.url().optional() })),
  },
  computerUse: { params: Empty, result: ComputerUse },
  setComputerUse: {
    params: named("SetComputerUseParams", z.object({ isEnabled: z.boolean() })),
    result: ComputerUse,
  },
  openPermissionSettings: {
    params: named("OpenPermissionSettingsParams", z.object({ permission: Permission })),
    result: Empty,
  },
} satisfies Record<z.infer<typeof RequestMethod>, { params: z.ZodType; result: z.ZodType }>;

export const EventName = named("EventName", z.enum(["openSettings"]));

/** Things the native app tells the page, such as a menu command. */
export const events = {
  openSettings: Empty,
} satisfies Record<z.infer<typeof EventName>, z.ZodType>;
