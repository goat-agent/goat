import * as storage from "./files/s3";
import { logger } from "./logger";

export class AvatarRejected extends Error {}

// The app scales pictures down before uploading, so this only stops abuse.
export const maxAvatarBytes = 1024 * 1024;

const log = logger.child({ module: "avatars" });

const formats = [
  { mediaType: "image/jpeg", magic: [0xff, 0xd8, 0xff] },
  { mediaType: "image/png", magic: [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a] },
];

function keyOf(avatarId: string): string {
  return `avatars/${avatarId}`;
}

function mediaTypeOf(bytes: Uint8Array): string | undefined {
  return formats.find(({ magic }) => magic.every((byte, i) => bytes[i] === byte))?.mediaType;
}

/** Stores a JPEG or PNG picture and returns its id. */
export async function store(file: File): Promise<string> {
  if (file.size > maxAvatarBytes) throw new AvatarRejected("picture too large");
  const bytes = new Uint8Array(await file.arrayBuffer());
  const mediaType = mediaTypeOf(bytes);
  if (!mediaType) throw new AvatarRejected("not a JPEG or PNG picture");

  const avatarId = crypto.randomUUID();
  await storage.put(keyOf(avatarId), bytes, mediaType);
  return avatarId;
}

export async function read(
  avatarId: string,
): Promise<{ bytes: Uint8Array<ArrayBuffer>; mediaType: string }> {
  const bytes = new Uint8Array(await storage.get(keyOf(avatarId)));
  return { bytes, mediaType: mediaTypeOf(bytes) ?? "application/octet-stream" };
}

/** Deletes a picture nothing points at anymore; a leftover object costs only storage. */
export async function discard(avatarId: string): Promise<void> {
  try {
    await storage.remove(keyOf(avatarId));
  } catch (error) {
    log.warn({ err: error, avatarId }, "could not delete avatar");
  }
}
